use super::{Engine, Error, abi::ABI_VERSION};
use crate::platform::prelude::*;
use core::fmt;
use std::sync::{Mutex, PoisonError};
use wasmtime::{
    Config, Instance, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

/// The most memory a plugin may use.
const MEMORY_LIMIT: usize = 256 << 20;
/// Roughly how many WebAssembly instructions a single call may run. This
/// stops a plugin that never finishes, after about a second or two.
const FUEL: u64 = 5_000_000_000;

/// Runs comparison plugins with [wasmtime](https://wasmtime.dev). If a call
/// fails, the plugin is started again from scratch for the next call, so a
/// plugin that crashed once doesn't stay broken.
pub struct WasmtimeEngine {
    module: Module,
    instance: Mutex<Option<Loaded>>,
}

struct Loaded {
    store: Store<StoreLimits>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    describe: TypedFunc<(i32, i32), i64>,
    generate: TypedFunc<(i32, i32), i64>,
}

impl fmt::Debug for WasmtimeEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmtimeEngine").finish_non_exhaustive()
    }
}

fn error(context: &str) -> impl Fn(wasmtime::Error) -> Error + '_ {
    move |e| Error(format!("{context}: {e}"))
}

impl WasmtimeEngine {
    /// Compiles the plugin and checks that it implements the ABI.
    pub fn new(module: &[u8]) -> Result<Self, Error> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = wasmtime::Engine::new(&config).map_err(error("The engine didn't start"))?;
        let module = Module::new(&engine, module).map_err(error("This isn't a valid plugin"))?;
        if module.imports().len() != 0 {
            return Err("Comparison plugins can't import anything.".into());
        }
        let this = Self {
            module,
            instance: Mutex::new(None),
        };
        *this.instance.lock().unwrap_or_else(PoisonError::into_inner) = Some(this.load()?);
        Ok(this)
    }

    fn load(&self) -> Result<Loaded, Error> {
        let limits = StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build();
        let mut store = Store::new(self.module.engine(), limits);
        store.limiter(|limits| limits);
        store
            .set_fuel(FUEL)
            .map_err(error("The plugin couldn't be started"))?;
        let instance = Instance::new(&mut store, &self.module, &[])
            .map_err(error("The plugin couldn't be started"))?;

        let version = instance
            .get_typed_func::<(), i32>(&mut store, "lscp_abi_version")
            .and_then(|f| f.call(&mut store, ()))
            .map_err(error("This isn't a comparison plugin"))?;
        if version != ABI_VERSION {
            return Err(Error(format!(
                "The plugin was made for version {version} of the plugin interface, but only version {ABI_VERSION} is supported."
            )));
        }

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| Error::from("The plugin doesn't export its memory."))?;
        let missing = error("The plugin is missing a function");
        Ok(Loaded {
            alloc: instance
                .get_typed_func(&mut store, "lscp_alloc")
                .map_err(&missing)?,
            describe: instance
                .get_typed_func(&mut store, "lscp_describe")
                .map_err(&missing)?,
            generate: instance
                .get_typed_func(&mut store, "lscp_generate")
                .map_err(&missing)?,
            memory,
            store,
        })
    }

    fn call(&self, generate: bool, input: &[u8]) -> Result<Vec<u8>, Error> {
        let mut instance = self.instance.lock().unwrap_or_else(PoisonError::into_inner);
        let loaded = match &mut *instance {
            Some(loaded) => loaded,
            None => instance.insert(self.load()?),
        };
        let result = loaded.call(generate, input);
        if result.is_err() {
            // The plugin may be in any state now. Start over next time.
            *instance = None;
        }
        result
    }
}

impl Loaded {
    fn call(&mut self, generate: bool, input: &[u8]) -> Result<Vec<u8>, Error> {
        let failed = error("The plugin failed");
        self.store.set_fuel(FUEL).map_err(&failed)?;
        let len = i32::try_from(input.len()).map_err(|_| Error::from("The run is too large."))?;
        let ptr = self.alloc.call(&mut self.store, len).map_err(&failed)?;
        self.memory
            .write(&mut self.store, ptr as u32 as usize, input)
            .map_err(|_| Error::from("The plugin gave out a buffer that doesn't fit."))?;

        let function = if generate {
            &self.generate
        } else {
            &self.describe
        };
        let packed = function
            .call(&mut self.store, (ptr, len))
            .map_err(&failed)? as u64;

        let (ptr, len) = ((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize);
        let data = self.memory.data(&self.store);
        let output = data
            .get(ptr..ptr.saturating_add(len))
            .ok_or_else(|| Error::from("The plugin returned a buffer that doesn't fit."))?;
        Ok(output.to_vec())
    }
}

impl Engine for WasmtimeEngine {
    fn describe(&self, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.call(false, input)
    }

    fn generate(&self, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.call(true, input)
    }
}
