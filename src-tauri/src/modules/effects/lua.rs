//! The Lua runtime: `mlua`, the `Amaz` API surface, and the script lifecycle.
//!
//! An effect's behaviour is a Lua class attached to an entity by a
//! `ScriptComponent`. `LUA_API_SPEC.md` counts the surface across 419 scripts;
//! the shape that matters is small and very consistent:
//!
//! ```lua
//! TintScript = {}                     -- the class named by `className`
//! function TintScript.new(construct)  -- the engine calls this with `true`
//!     local self = setmetatable({}, {__index = TintScript})
//!     if construct then self:constructor() end
//!     return self
//! end
//! function TintScript:onStart(comp)   -- cache the material here
//! function TintScript:onUpdate(comp, deltaTime)
//! function TintScript:seekToTime(comp, time)
//! function TintScript:onEvent(sys, event)
//! function TintScript:onDestroy(comp)
//! ```
//!
//! ## What a script actually does, and why that shapes this file
//!
//! Almost all of it is *writing uniforms*: 726 `setFloat`, 612 `setInt` and
//! 261 `setTex` calls across the corpus, against 33 calls to anything
//! structural. So [`MaterialHandle`] — a shared, Rust-side uniform table that
//! Lua writes and [`super::graph`] reads — is the load-bearing binding, and
//! everything else is scaffolding around it.
//!
//! Two details of that binding are easy to get wrong and are covered by tests:
//!
//! - **`material["name"] = value` is the same thing as `setFloat`/`setInt`,**
//!   dispatched on the Lua value's type. Scripts use both forms freely, often
//!   in adjacent lines. It needs a `__newindex` metamethod, not a field.
//! - **`Amaz.BuiltinObject:getInputTextureWidth()` and
//!   `Amaz.BuiltinObject.getInputTextureWidth()` both appear**, 33 and 21
//!   times. The function ignores `self`, so the binding has to accept a
//!   leading argument that may or may not be there.
//!
//! ## Module composition
//!
//! Scripts compose with a host-provided `includeRelativePath(name)`, never with
//! `require` — there are zero `require(` calls in the corpus. It resolves
//! against the directory of the script that is currently executing, so the
//! runtime keeps a stack of them.

use mlua::{Lua, MetaMethod, MultiValue, UserData, UserDataMethods, Value, Variadic};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("{script}: {message}")]
    Lua { script: String, message: String },
    #[error("{script} declares no class named {class}")]
    NoSuchClass { script: String, class: String },
    #[error("{script}: {class}.new is missing or is not a function")]
    NoConstructor { script: String, class: String },
}

pub type Result<T> = std::result::Result<T, ScriptError>;

// ---------------------------------------------------------------------------
// The uniform table Lua writes and the render graph reads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialValues {
    pub floats: BTreeMap<String, f32>,
    pub ints: BTreeMap<String, i64>,
    pub vec2: BTreeMap<String, [f32; 2]>,
    pub vec3: BTreeMap<String, [f32; 3]>,
    pub vec4: BTreeMap<String, [f32; 4]>,
    /// Uniform name → whatever the script named. A path inside the package, a
    /// `share://` URI, or the id of a texture the host pre-bound.
    pub textures: BTreeMap<String, String>,
    pub macros: BTreeMap<String, i64>,
}

/// A material, shared between the Lua VM and the render graph.
#[derive(Debug, Clone)]
pub struct MaterialHandle {
    pub name: String,
    pub values: Arc<Mutex<MaterialValues>>,
}

impl MaterialHandle {
    pub fn new(name: impl Into<String>, values: MaterialValues) -> Self {
        Self {
            name: name.into(),
            values: Arc::new(Mutex::new(values)),
        }
    }

    pub fn snapshot(&self) -> MaterialValues {
        self.values.lock().clone()
    }
}

/// Dispatch a bare `material["x"] = v` by the Lua value's type, which is what
/// the engine does: an integral number becomes an int, anything else numeric
/// becomes a float, and a vector userdata goes to its own map.
fn assign(values: &mut MaterialValues, name: String, value: &Value) -> mlua::Result<()> {
    match value {
        Value::Integer(v) => {
            values.ints.insert(name, *v);
        }
        Value::Number(v) => {
            values.floats.insert(name, *v as f32);
        }
        Value::Boolean(v) => {
            values.ints.insert(name, i64::from(*v));
        }
        Value::String(v) => {
            values.textures.insert(name, v.to_str()?.to_string());
        }
        Value::UserData(data) => {
            if let Ok(v) = data.borrow::<Vec2>() {
                values.vec2.insert(name, [v.x, v.y]);
            } else if let Ok(v) = data.borrow::<Vec3>() {
                values.vec3.insert(name, [v.x, v.y, v.z]);
            } else if let Ok(v) = data.borrow::<Vec4>() {
                values.vec4.insert(name, [v.x, v.y, v.z, v.w]);
            } else if let Ok(v) = data.borrow::<TextureRef>() {
                values.textures.insert(name, v.0.clone());
            }
        }
        _ => {}
    }
    Ok(())
}

impl UserData for MaterialHandle {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("setFloat", |_, this, (name, value): (String, f64)| {
            this.values.lock().floats.insert(name, value as f32);
            Ok(())
        });
        methods.add_method("setInt", |_, this, (name, value): (String, i64)| {
            this.values.lock().ints.insert(name, value);
            Ok(())
        });
        // The typed setters go through the same dispatch as the bracket
        // shortcut rather than taking a `Vec3` directly: `Vec3` is userdata, and
        // a script that passed the wrong arity would otherwise get a type error
        // from mlua instead of the engine's own silent no-op.
        for name in ["setVec2", "setVec3", "setVec4", "setMatrix"] {
            methods.add_method(name, |_, this, (name, value): (String, Value)| {
                let mut values = this.values.lock();
                assign(&mut values, name, &value)
            });
        }
        // `tex` is a Texture userdata in most scripts and an asset path string
        // in some; both are accepted because both are in the corpus.
        methods.add_method("setTex", |_, this, (name, value): (String, Value)| {
            let mut values = this.values.lock();
            assign(&mut values, name, &value)
        });
        methods.add_method("enableMacro", |_, this, (name, value): (String, i64)| {
            this.values.lock().macros.insert(name, value);
            Ok(())
        });
        methods.add_method("getFloat", |_, this, name: String| {
            Ok(this.values.lock().floats.get(&name).copied().unwrap_or(0.0))
        });

        // The bracket shortcut. `self.pass3Material["time"] = ...` is not a
        // field assignment, it is `setFloat`/`setInt` chosen by value type.
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, this, (name, value): (String, Value)| {
                let mut values = this.values.lock();
                assign(&mut values, name, &value)
            },
        );
    }
}

/// What `setTex` is handed when the script got a texture from the host.
#[derive(Debug, Clone)]
pub struct TextureRef(pub String);

impl UserData for TextureRef {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.0.clone()));
    }
}

// ---------------------------------------------------------------------------
// Math types
// ---------------------------------------------------------------------------

macro_rules! vector_type {
    ($name:ident, $($field:ident),+) => {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct $name { $(pub $field: f32),+ }

        impl UserData for $name {
            fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
                $(
                    fields.add_field_method_get(stringify!($field), |_, this| Ok(this.$field));
                    fields.add_field_method_set(stringify!($field), |_, this, v: f32| {
                        this.$field = v;
                        Ok(())
                    });
                )+
            }

            fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
                // Scripts write `self.scale / 100.0` and `a + b` freely, so
                // both the vector/vector and the vector/scalar form of each
                // operator has to exist.
                methods.add_meta_method(MetaMethod::Add, |_, this, other: Value| {
                    Ok(binary(*this, &other, |a, b| a + b))
                });
                methods.add_meta_method(MetaMethod::Sub, |_, this, other: Value| {
                    Ok(binary(*this, &other, |a, b| a - b))
                });
                methods.add_meta_method(MetaMethod::Mul, |_, this, other: Value| {
                    Ok(binary(*this, &other, |a, b| a * b))
                });
                methods.add_meta_method(MetaMethod::Div, |_, this, other: Value| {
                    Ok(binary(*this, &other, |a, b| a / b))
                });
            }
        }

        impl $name {
            fn components(&self) -> Vec<f32> { vec![$(self.$field),+] }
            fn from_components(values: &[f32]) -> Self {
                let mut iter = values.iter().copied();
                Self { $($field: iter.next().unwrap_or(0.0)),+ }
            }
        }

        fn binary(this: $name, other: &Value, op: impl Fn(f32, f32) -> f32) -> $name {
            let mine = this.components();
            let theirs: Vec<f32> = match other {
                Value::Integer(v) => vec![*v as f32; mine.len()],
                Value::Number(v) => vec![*v as f32; mine.len()],
                Value::UserData(data) => match data.borrow::<$name>() {
                    Ok(v) => v.components(),
                    Err(_) => return this,
                },
                _ => return this,
            };
            let combined: Vec<f32> = mine
                .iter()
                .zip(theirs.iter())
                .map(|(a, b)| op(*a, *b))
                .collect();
            $name::from_components(&combined)
        }
    };
}

mod vec2 {
    use super::*;
    vector_type!(Vec2, x, y);
}
mod vec3 {
    use super::*;
    vector_type!(Vec3, x, y, z);
}
mod vec4 {
    use super::*;
    vector_type!(Vec4, x, y, z, w);
}
mod color {
    use super::*;
    vector_type!(Color, r, g, b, a);
}

pub use color::Color;
pub use vec2::Vec2;
pub use vec3::Vec3;
pub use vec4::Vec4;

// ---------------------------------------------------------------------------
// Host state the API reads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct HostState {
    pub input_width: u32,
    pub input_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    /// `Amaz.BuiltinObject.getUserTexture("#TransitionInput0")` and friends.
    /// The `#` prefix marks a slot the host pre-bound.
    pub user_textures: BTreeMap<String, String>,
    /// `Amaz.Input.frameTimestamp`, 0..1 through the effect's own duration.
    pub frame_timestamp: f32,
}

// ---------------------------------------------------------------------------
// The runtime
// ---------------------------------------------------------------------------

pub struct LuaRuntime {
    lua: Lua,
    host: Arc<Mutex<HostState>>,
    /// The directory of the script currently executing, for
    /// `includeRelativePath`. A stack because a module can include another.
    #[allow(dead_code)]
    include_root: Arc<Mutex<Vec<String>>>,
}

impl LuaRuntime {
    pub fn new() -> Result<Self> {
        let lua = Lua::new();
        let host = Arc::new(Mutex::new(HostState::default()));
        let include_root = Arc::new(Mutex::new(Vec::new()));

        let runtime = Self {
            lua,
            host,
            include_root,
        };
        runtime.register_amaz()?;
        Ok(runtime)
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn set_host(&self, state: HostState) {
        *self.host.lock() = state;
    }

    pub fn host(&self) -> HostState {
        self.host.lock().clone()
    }

    fn err(&self, script: &str, error: mlua::Error) -> ScriptError {
        ScriptError::Lua {
            script: script.to_string(),
            message: error.to_string(),
        }
    }

    fn register_amaz(&self) -> Result<()> {
        let lua = &self.lua;
        let build = || -> mlua::Result<()> {
            let amaz = lua.create_table()?;

            amaz.set(
                "Vector2f",
                lua.create_function(|_, (x, y): (f32, f32)| Ok(Vec2 { x, y }))?,
            )?;
            amaz.set(
                "Vector3f",
                lua.create_function(|_, (x, y, z): (f32, f32, f32)| Ok(Vec3 { x, y, z }))?,
            )?;
            amaz.set(
                "Vector4f",
                lua.create_function(|_, (x, y, z, w): (f32, f32, f32, f32)| {
                    Ok(Vec4 { x, y, z, w })
                })?,
            )?;
            amaz.set(
                "Color",
                lua.create_function(|_, (r, g, b, a): (f32, f32, f32, Option<f32>)| {
                    Ok(Color {
                        r,
                        g,
                        b,
                        a: a.unwrap_or(1.0),
                    })
                })?,
            )?;

            // The message argument is sometimes a number — the engine
            // stringifies it — so it is taken as a Value rather than a String.
            for (name, level) in [
                ("LOGI", tracing::Level::INFO),
                ("LOGE", tracing::Level::ERROR),
                ("LOGW", tracing::Level::WARN),
                ("LOGS", tracing::Level::DEBUG),
            ] {
                amaz.set(
                    name,
                    lua.create_function(move |_, (tag, message): (String, Value)| {
                        let text = match &message {
                            Value::String(s) => s.to_string_lossy().to_string(),
                            other => format!("{other:?}"),
                        };
                        match level {
                            tracing::Level::ERROR => {
                                tracing::error!(target: "effect", %tag, "{text}")
                            }
                            tracing::Level::WARN => {
                                tracing::warn!(target: "effect", %tag, "{text}")
                            }
                            tracing::Level::INFO => {
                                tracing::info!(target: "effect", %tag, "{text}")
                            }
                            _ => tracing::debug!(target: "effect", %tag, "{text}"),
                        }
                        Ok(())
                    })?,
                )?;
            }

            // `Amaz.BuiltinObject`. Every getter is called both as `:foo()` and
            // as `.foo()`, so each takes a variadic and ignores it — which is
            // exactly what the engine's own binding does.
            let builtin = lua.create_table()?;
            for (name, pick) in [
                ("getInputTextureWidth", 0usize),
                ("getInputTextureHeight", 1),
                ("getOutputTextureWidth", 2),
                ("getOutputTextureHeight", 3),
            ] {
                let host = Arc::clone(&self.host);
                builtin.set(
                    name,
                    lua.create_function(move |_, _: Variadic<Value>| {
                        let state = host.lock();
                        Ok(match pick {
                            0 => state.input_width,
                            1 => state.input_height,
                            2 => state.output_width.max(state.input_width),
                            _ => state.output_height.max(state.input_height),
                        })
                    })?,
                )?;
            }
            let host = Arc::clone(&self.host);
            builtin.set(
                "getUserTexture",
                lua.create_function(move |_, args: Variadic<Value>| {
                    // Called as `.getUserTexture(name)` and as
                    // `:getUserTexture(name)`; the name is the last argument
                    // either way.
                    let name = args.iter().rev().find_map(lua_string).unwrap_or_default();
                    let state = host.lock();
                    Ok(TextureRef(
                        state.user_textures.get(&name).cloned().unwrap_or(name),
                    ))
                })?,
            )?;
            amaz.set("BuiltinObject", builtin)?;

            let input = lua.create_table()?;
            input.set("frameTimestamp", 0.0f32)?;
            amaz.set("Input", input)?;

            // `Amaz.Macros.EditorSDK` gates editor-only branches. We are not
            // the editor, and a script that thinks we are takes paths that
            // reference tooling that does not exist here.
            let macros = lua.create_table()?;
            macros.set("EditorSDK", false)?;
            amaz.set("Macros", macros)?;

            let event_type = lua.create_table()?;
            event_type.set("SetEffectIntensity", 1i64)?;
            amaz.set("AppEventType", event_type)?;

            lua.globals().set("Amaz", amaz)?;
            Ok(())
        };
        build().map_err(|e| self.err("<Amaz bindings>", e))
    }

    /// Execute a script file's top level, which is what defines its class.
    pub fn load_script(&self, path: &str, source: &str) -> Result<()> {
        self.lua
            .load(source)
            .set_name(path)
            .exec()
            .map_err(|e| self.err(path, e))
    }

    /// Instantiate a class: `<ClassName>.new(true)`.
    pub fn instantiate(&self, path: &str, class: &str) -> Result<ScriptInstance> {
        let globals = self.lua.globals();
        let table: mlua::Table = globals.get(class).map_err(|_| ScriptError::NoSuchClass {
            script: path.to_string(),
            class: class.to_string(),
        })?;
        let new: mlua::Function = table.get("new").map_err(|_| ScriptError::NoConstructor {
            script: path.to_string(),
            class: class.to_string(),
        })?;
        // The engine passes `true` for `construct`, which is the flag `.new`
        // tests before calling `:constructor()`.
        let instance: mlua::Table = new.call(true).map_err(|e| self.err(path, e))?;
        Ok(ScriptInstance {
            script: path.to_string(),
            class: class.to_string(),
            table: instance,
        })
    }

    /// The `comp` userdata every hook is handed: the `ScriptComponent` that
    /// hosts the script. `comp.properties` is the parameter table.
    pub fn make_component(
        &self,
        entity_name: &str,
        material: Option<MaterialHandle>,
        properties: BTreeMap<String, Value>,
    ) -> Result<mlua::Table> {
        let build = || -> mlua::Result<mlua::Table> {
            let comp = self.lua.create_table()?;

            let props = self.lua.create_table()?;
            for (key, value) in properties {
                props.set(key, value)?;
            }
            comp.set("properties", props)?;

            let entity = self.lua.create_table()?;
            entity.set("name", entity_name)?;
            entity.set("visible", true)?;
            entity.set("layer", 0)?;

            // `entity:getComponent("MeshRenderer").material` is how a script
            // reaches the material in nearly every case, so the one component
            // that exists is a mesh renderer.
            let renderer = self.lua.create_table()?;
            if let Some(material) = material.clone() {
                renderer.set("material", material)?;
            }
            let renderer_for_get = renderer.clone();
            entity.set(
                "getComponent",
                self.lua.create_function(move |_, args: Variadic<Value>| {
                    // Called both as `entity:getComponent("X")` and as
                    // `entity.getComponent("X")`, so the type name is the
                    // last string argument either way.
                    let kind = args.iter().rev().find_map(lua_string).unwrap_or_default();
                    Ok(match kind.as_str() {
                        "MeshRenderer" | "Renderer" => Value::Table(renderer_for_get.clone()),
                        // A component this single-entity scene does not
                        // have. Returning nil is what the engine does, and
                        // it is what lets a script's `if not x then` guard
                        // work rather than erroring.
                        _ => Value::Nil,
                    })
                })?,
            )?;
            // A single-entity scene: searching finds only this entity, and a
            // script that wants another gets nil rather than a wrong one.
            entity.set(
                "searchEntity",
                self.lua
                    .create_function(|_, _: Variadic<Value>| Ok(Value::Nil))?,
            )?;
            comp.set("entity", entity)?;
            Ok(comp)
        };
        build().map_err(|e| self.err("<component>", e))
    }

    /// The `event` userdata: `event.args:get(i)` and `event.args:size()`, which
    /// is how every host parameter change arrives.
    pub fn make_event(&self, args: Vec<Value>) -> Result<mlua::Table> {
        let build = || -> mlua::Result<mlua::Table> {
            let event = self.lua.create_table()?;
            let stored = self.lua.create_table()?;
            for (i, value) in args.iter().enumerate() {
                stored.set(i + 1, value.clone())?;
            }

            let vector = self.lua.create_table()?;
            let for_get = stored.clone();
            vector.set(
                "get",
                self.lua.create_function(move |_, args: Variadic<Value>| {
                    // Zero-indexed on the Lua side, one-indexed in the
                    // table that backs it.
                    let index = args.iter().rev().find_map(|v| v.as_integer()).unwrap_or(0);
                    for_get.get::<Value>(index + 1)
                })?,
            )?;
            let for_size = stored.clone();
            vector.set(
                "size",
                self.lua
                    .create_function(move |_, _: Variadic<Value>| Ok(for_size.raw_len()))?,
            )?;
            event.set("args", vector)?;
            event.set("type", 1i64)?;
            Ok(event)
        };
        build().map_err(|e| self.err("<event>", e))
    }
}

/// One instantiated script, and the lifecycle calls that drive it.
///
/// Every hook is optional. A script that defines only `onUpdate` is normal —
/// `onStart` appears 183 times and `onDestroy` 52 across 419 scripts — so a
/// missing hook is silently skipped rather than being an error.
pub struct ScriptInstance {
    pub script: String,
    pub class: String,
    table: mlua::Table,
}

impl ScriptInstance {
    pub fn has_hook(&self, name: &str) -> bool {
        matches!(self.table.get::<Value>(name), Ok(Value::Function(_)))
    }

    fn call(&self, name: &str, args: MultiValue) -> Result<()> {
        let Ok(Value::Function(hook)) = self.table.get::<Value>(name) else {
            return Ok(());
        };
        let mut all = MultiValue::new();
        all.push_front(Value::Table(self.table.clone()));
        for value in args {
            all.push_back(value);
        }
        hook.call::<()>(all).map_err(|e| ScriptError::Lua {
            script: format!("{}:{name}", self.script),
            message: e.to_string(),
        })
    }

    pub fn on_start(&self, comp: &mlua::Table) -> Result<()> {
        self.call("onStart", multi([Value::Table(comp.clone())]))
    }

    pub fn on_update(&self, comp: &mlua::Table, delta_seconds: f32) -> Result<()> {
        self.call(
            "onUpdate",
            multi([
                Value::Table(comp.clone()),
                Value::Number(delta_seconds as f64),
            ]),
        )
    }

    pub fn seek_to_time(&self, comp: &mlua::Table, seconds: f32) -> Result<()> {
        self.call(
            "seekToTime",
            multi([Value::Table(comp.clone()), Value::Number(seconds as f64)]),
        )
    }

    pub fn on_event(&self, comp: &mlua::Table, event: &mlua::Table) -> Result<()> {
        self.call(
            "onEvent",
            multi([Value::Table(comp.clone()), Value::Table(event.clone())]),
        )
    }

    pub fn on_destroy(&self, comp: &mlua::Table) -> Result<()> {
        self.call("onDestroy", multi([Value::Table(comp.clone())]))
    }

    pub fn get<T: mlua::FromLua>(&self, name: &str) -> Option<T> {
        self.table.get(name).ok()
    }

    pub fn set<T: mlua::IntoLua>(&self, name: &str, value: T) -> Result<()> {
        self.table.set(name, value).map_err(|e| ScriptError::Lua {
            script: self.script.clone(),
            message: e.to_string(),
        })
    }
}

/// `Value::as_str` borrows and the borrow cannot outlive the iteration, so the
/// string is taken by value. Used wherever an argument list has to be searched
/// for a name because the same function is called both as `:f(x)` and `.f(x)`.
fn lua_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.to_string_lossy().to_string()),
        _ => None,
    }
}

fn multi<const N: usize>(values: [Value; N]) -> MultiValue {
    let mut out = MultiValue::new();
    for value in values {
        out.push_back(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script in the shape the corpus uses: a global class table, a `.new`
    /// that takes the `construct` flag, and hooks that write uniforms.
    const SCRIPT: &str = r#"
TintScript = {}
TintScript.__index = TintScript

function TintScript.new(construct)
    local self = setmetatable({}, TintScript)
    self.curTime = 0
    self.intensity = 1.0
    if construct then self:constructor() end
    return self
end

function TintScript:constructor()
    self.constructed = true
end

function TintScript:onStart(comp)
    self.material = comp.entity:getComponent("MeshRenderer").material
    self.width = Amaz.BuiltinObject:getInputTextureWidth()
    self.heightDotCall = Amaz.BuiltinObject.getInputTextureHeight()
    self.material:setFloat("u_intensity", self.intensity)
end

function TintScript:onUpdate(comp, deltaTime)
    self.curTime = self.curTime + deltaTime
    self:seekToTime(comp, self.curTime)
end

function TintScript:seekToTime(comp, time)
    self.material["u_time"] = time
    self.material["u_mode"] = 2
    self.material:setVec3("u_tint", Amaz.Vector3f(1.0, 0.5, 0.25))
end

function TintScript:onEvent(sys, event)
    if event.args:get(0) == "effects_adjust_intensity" then
        self.intensity = event.args:get(1)
        self.material:setFloat("u_intensity", self.intensity)
    end
end

function TintScript:onDestroy(comp)
    self.destroyed = true
end
"#;

    fn runtime_with_script() -> (LuaRuntime, ScriptInstance, mlua::Table, MaterialHandle) {
        let runtime = LuaRuntime::new().unwrap();
        runtime.set_host(HostState {
            input_width: 1080,
            input_height: 1920,
            ..Default::default()
        });
        runtime.load_script("lua/TintScript.lua", SCRIPT).unwrap();
        let instance = runtime
            .instantiate("lua/TintScript.lua", "TintScript")
            .unwrap();
        let material = MaterialHandle::new("tint", MaterialValues::default());
        let comp = runtime
            .make_component("Quad", Some(material.clone()), BTreeMap::new())
            .unwrap();
        (runtime, instance, comp, material)
    }

    #[test]
    fn new_runs_the_constructor_when_the_engine_passes_true() {
        let (_runtime, instance, _comp, _material) = runtime_with_script();
        assert_eq!(instance.get::<bool>("constructed"), Some(true));
    }

    #[test]
    fn on_start_reaches_the_material_through_the_mesh_renderer() {
        let (_runtime, instance, comp, material) = runtime_with_script();
        instance.on_start(&comp).unwrap();
        assert_eq!(material.snapshot().floats["u_intensity"], 1.0);
    }

    #[test]
    fn builtin_object_answers_to_both_the_colon_and_the_dot_call() {
        // 33 sites use `:`, 21 use `.`. A binding that only handles one gets a
        // width of nil in a fifth of the corpus.
        let (_runtime, instance, comp, _material) = runtime_with_script();
        instance.on_start(&comp).unwrap();
        assert_eq!(instance.get::<u32>("width"), Some(1080));
        assert_eq!(instance.get::<u32>("heightDotCall"), Some(1920));
    }

    #[test]
    fn the_bracket_shortcut_dispatches_on_the_lua_value_type() {
        // `material["u_time"] = time` is setFloat; `= 2` is setInt. Getting
        // this wrong writes an integer into a float uniform, which is a
        // wrong picture rather than an error.
        let (_runtime, instance, comp, material) = runtime_with_script();
        instance.on_start(&comp).unwrap();
        instance.on_update(&comp, 0.5).unwrap();

        let values = material.snapshot();
        assert_eq!(values.floats["u_time"], 0.5);
        assert_eq!(values.ints["u_mode"], 2);
        assert!(!values.floats.contains_key("u_mode"));
        assert_eq!(values.vec3["u_tint"], [1.0, 0.5, 0.25]);
    }

    #[test]
    fn on_update_accumulates_time_and_drives_seek_to_time() {
        let (_runtime, instance, comp, material) = runtime_with_script();
        instance.on_start(&comp).unwrap();
        instance.on_update(&comp, 0.25).unwrap();
        instance.on_update(&comp, 0.25).unwrap();
        assert_eq!(material.snapshot().floats["u_time"], 0.5);
    }

    #[test]
    fn a_host_slider_arrives_as_a_keyed_event() {
        // §8.3: the host broadcasts `effect_key` and value, and the script
        // matches on `event.args:get(0)`.
        let (runtime, instance, comp, material) = runtime_with_script();
        instance.on_start(&comp).unwrap();

        let event = runtime
            .make_event(vec![
                Value::String(
                    runtime
                        .lua()
                        .create_string("effects_adjust_intensity")
                        .unwrap(),
                ),
                Value::Number(0.35),
            ])
            .unwrap();
        instance.on_event(&comp, &event).unwrap();
        assert!((material.snapshot().floats["u_intensity"] - 0.35).abs() < 1e-6);
    }

    #[test]
    fn an_unrelated_event_key_is_ignored_rather_than_applied() {
        let (runtime, instance, comp, material) = runtime_with_script();
        instance.on_start(&comp).unwrap();
        let event = runtime
            .make_event(vec![
                Value::String(runtime.lua().create_string("something_else").unwrap()),
                Value::Number(0.0),
            ])
            .unwrap();
        instance.on_event(&comp, &event).unwrap();
        assert_eq!(material.snapshot().floats["u_intensity"], 1.0);
    }

    #[test]
    fn a_missing_hook_is_skipped_rather_than_being_an_error() {
        let runtime = LuaRuntime::new().unwrap();
        runtime
            .load_script(
                "lua/Bare.lua",
                "Bare = {}\nfunction Bare.new(c) return setmetatable({}, {__index = Bare}) end\n",
            )
            .unwrap();
        let instance = runtime.instantiate("lua/Bare.lua", "Bare").unwrap();
        let comp = runtime
            .make_component("Quad", None, BTreeMap::new())
            .unwrap();
        assert!(!instance.has_hook("onUpdate"));
        instance.on_start(&comp).unwrap();
        instance.on_update(&comp, 0.016).unwrap();
        instance.on_destroy(&comp).unwrap();
    }

    #[test]
    fn vector_arithmetic_works_against_both_vectors_and_scalars() {
        // `self.scale / 100.0` is idiomatic in the corpus and would silently
        // produce nil without the scalar form of the metamethod.
        let runtime = LuaRuntime::new().unwrap();
        runtime
            .load_script(
                "test",
                r#"
                local a = Amaz.Vector3f(3, 6, 9)
                local b = Amaz.Vector3f(1, 2, 3)
                local sum = a + b
                local scaled = a / 3.0
                result = {sum.x, sum.y, sum.z, scaled.x, scaled.y, scaled.z}
                "#,
            )
            .unwrap();
        let result: Vec<f32> = runtime.lua().globals().get("result").unwrap();
        assert_eq!(result, [4.0, 8.0, 12.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_lua_error_names_the_script_and_the_hook() {
        let runtime = LuaRuntime::new().unwrap();
        runtime
            .load_script(
                "lua/Broken.lua",
                "Broken = {}\n\
                 function Broken.new(c) return setmetatable({}, {__index = Broken}) end\n\
                 function Broken:onStart(comp) error('deliberate') end\n",
            )
            .unwrap();
        let instance = runtime.instantiate("lua/Broken.lua", "Broken").unwrap();
        let comp = runtime
            .make_component("Quad", None, BTreeMap::new())
            .unwrap();
        let error = instance.on_start(&comp).unwrap_err().to_string();
        assert!(error.contains("lua/Broken.lua:onStart"), "{error}");
        assert!(error.contains("deliberate"), "{error}");
    }
}
