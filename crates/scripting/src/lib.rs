//! Luau scripting for gameplay content. Build spike: proves the VM, its sandbox and its
//! limits on every target before the host API is designed on top.
#[cfg(test)]
mod tests {
    use mlua::{Lua, Value, VmState};

    #[test]
    fn typed_function_runs_sandboxed_with_limits() {
        let lua = Lua::new();
        lua.sandbox(true).unwrap();
        lua.set_memory_limit(8 * 1024 * 1024).unwrap();
        let chunk = r#"
            type Context = { keys: number, quest: string }
            local function ready(ctx: Context): boolean
                return ctx.keys >= 1 and ctx.quest == "active"
            end
            return ready
        "#;
        let ready: mlua::Function = lua.load(chunk).eval().unwrap();
        let ctx = lua.create_table().unwrap();
        ctx.set("keys", 1).unwrap();
        ctx.set("quest", "active").unwrap();
        assert!(ready.call::<bool>(ctx).unwrap());
        // No file or process access inside the sandbox.
        assert!(lua.load("return io").eval::<Value>().unwrap().is_nil());
        assert!(
            lua.load("return os.execute")
                .eval::<Value>()
                .unwrap()
                .is_nil()
        );
        // A runaway script is stopped by the host.
        let budget = std::sync::atomic::AtomicU32::new(1000);
        lua.set_interrupt(move |_| {
            if budget.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) == 1 {
                return Err(mlua::Error::runtime("script exceeded its budget"));
            }
            Ok(VmState::Continue)
        });
        let error = lua
            .load("while true do end")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(error.contains("budget"), "{error}");
    }
}
