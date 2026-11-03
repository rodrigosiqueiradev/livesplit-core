use super::{Engine, Error, abi::ABI_VERSION};
use crate::platform::prelude::*;
use core::{cell::RefCell, fmt};
use js_sys::{BigInt, Function, Object, Reflect, Uint8Array, WebAssembly};
use wasm_bindgen::{JsCast, JsValue};

/// Runs comparison plugins with the browser's own WebAssembly engine.
///
/// Unlike [`WasmtimeEngine`](super::WasmtimeEngine), a call can't be
/// interrupted, so a plugin that never finishes freezes the page. Only run
/// plugins the runner chose to add. If a call fails, the plugin is started
/// again from scratch for the next call.
pub struct WebEngine {
    module: WebAssembly::Module,
    instance: RefCell<Option<Loaded>>,
}

// SAFETY: Without the atomics target feature, WebAssembly in the browser is
// single threaded, so the JavaScript objects are never shared between
// threads.
#[cfg(not(target_feature = "atomics"))]
unsafe impl Send for WebEngine {}
// SAFETY: See above.
#[cfg(not(target_feature = "atomics"))]
unsafe impl Sync for WebEngine {}

struct Loaded {
    memory: WebAssembly::Memory,
    alloc: Function,
    describe: Function,
    generate: Function,
}

impl fmt::Debug for WebEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebEngine").finish_non_exhaustive()
    }
}

fn js_error(context: &str) -> impl Fn(JsValue) -> Error + '_ {
    move |e| {
        let message = e
            .dyn_ref::<js_sys::Error>()
            .map(|e| String::from(e.message()))
            .or_else(|| e.as_string())
            .unwrap_or_default();
        if message.is_empty() {
            Error(context.into())
        } else {
            Error(format!("{context}: {message}"))
        }
    }
}

fn export<T: JsCast>(exports: &Object, name: &str) -> Result<T, Error> {
    Reflect::get(exports, &JsValue::from_str(name))
        .ok()
        .and_then(|value| value.dyn_into::<T>().ok())
        .ok_or_else(|| Error(format!("The plugin is missing '{name}'.")))
}

impl WebEngine {
    /// Compiles the plugin and checks that it implements the ABI. Browsers
    /// only compile small modules synchronously on the main thread (Chrome
    /// allows up to 8 MB). For larger ones, compile the module asynchronously
    /// and use [`from_module`](Self::from_module).
    pub fn new(module: &[u8]) -> Result<Self, Error> {
        let module = WebAssembly::Module::new(&Uint8Array::from(module).into())
            .map_err(js_error("This isn't a valid plugin"))?;
        Self::from_module(module)
    }

    /// Uses a module that was already compiled.
    pub fn from_module(module: WebAssembly::Module) -> Result<Self, Error> {
        if WebAssembly::Module::imports(&module).length() != 0 {
            return Err("Comparison plugins can't import anything.".into());
        }
        let this = Self {
            module,
            instance: RefCell::new(None),
        };
        *this.instance.borrow_mut() = Some(this.load()?);
        Ok(this)
    }

    fn load(&self) -> Result<Loaded, Error> {
        let instance = WebAssembly::Instance::new(&self.module, &Object::new())
            .map_err(js_error("The plugin couldn't be started"))?;
        let exports = instance.exports();

        let version = export::<Function>(&exports, "lscp_abi_version")
            .map_err(|_| Error::from("This isn't a comparison plugin."))?
            .call0(&JsValue::UNDEFINED)
            .map_err(js_error("This isn't a comparison plugin"))?
            .as_f64()
            .unwrap_or_default();
        if version != f64::from(ABI_VERSION) {
            return Err(Error(format!(
                "The plugin was made for version {version} of the plugin interface, but only version {ABI_VERSION} is supported."
            )));
        }

        Ok(Loaded {
            memory: export(&exports, "memory")?,
            alloc: export(&exports, "lscp_alloc")?,
            describe: export(&exports, "lscp_describe")?,
            generate: export(&exports, "lscp_generate")?,
        })
    }

    fn call(&self, generate: bool, input: &[u8]) -> Result<Vec<u8>, Error> {
        let mut instance = self.instance.borrow_mut();
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
    fn bytes(&self) -> Uint8Array {
        // The buffer changes whenever the memory grows, so get it every time.
        Uint8Array::new(&self.memory.buffer())
    }

    fn call(&self, generate: bool, input: &[u8]) -> Result<Vec<u8>, Error> {
        let failed = js_error("The plugin failed");
        let len = u32::try_from(input.len())
            .ok()
            .filter(|&len| len <= i32::MAX as u32)
            .ok_or_else(|| Error::from("The run is too large."))?;
        let ptr = self
            .alloc
            .call1(&JsValue::UNDEFINED, &JsValue::from(len))
            .map_err(&failed)?
            .as_f64()
            .ok_or_else(|| Error::from("The plugin gave out no buffer."))? as u32;

        let memory = self.bytes();
        if u64::from(ptr) + u64::from(len) > u64::from(memory.length()) {
            return Err("The plugin gave out a buffer that doesn't fit.".into());
        }
        memory.subarray(ptr, ptr + len).copy_from(input);

        let function = if generate {
            &self.generate
        } else {
            &self.describe
        };
        let packed = function
            .call2(
                &JsValue::UNDEFINED,
                &JsValue::from(ptr),
                &JsValue::from(len),
            )
            .map_err(&failed)?;
        let packed = packed
            .dyn_into::<BigInt>()
            .ok()
            .and_then(|packed| i64::try_from(packed).ok())
            .ok_or_else(|| Error::from("The plugin returned something unreadable."))?
            as u64;

        let (ptr, len) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
        let memory = self.bytes();
        if u64::from(ptr) + u64::from(len) > u64::from(memory.length()) {
            return Err("The plugin returned a buffer that doesn't fit.".into());
        }
        Ok(memory.subarray(ptr, ptr + len).to_vec())
    }
}

impl Engine for WebEngine {
    fn describe(&self, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.call(false, input)
    }

    fn generate(&self, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.call(true, input)
    }
}
