//! TypeScript → JavaScript, on the host, before code enters the sandbox.
//!
//! Uses swc's strip-only mode, which blanks type annotations with whitespace,
//! so every line/column in a runtime error points at what the model actually
//! wrote. Syntax that needs real codegen (`enum`, `namespace`, parameter
//! properties) falls back to transform mode.

#[cfg(feature = "typescript")]
pub(crate) fn strip(code: &str, filename: &str) -> Result<String, String> {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use swc_common::errors::{HANDLER, Handler};
    use swc_common::sync::Lrc;
    use swc_common::{GLOBALS, Globals, SourceMap};
    use swc_ts_fast_strip::{ErrorCode, Mode, Options, operate};

    // Wrap in an async function so top-level `await` and `return` parse. The
    // prefix shares line 1 with the user's code, keeping line numbers aligned.
    const PREFIX: &str = "async function __modebox__() {";
    const SUFFIX: &str = "\n}";

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let wrapped = format!("{PREFIX}{code}{SUFFIX}");
    let attempt = |mode: Mode| -> Result<String, (Option<ErrorCode>, String)> {
        let cm: Lrc<SourceMap> = Default::default();
        let buf = Buf::default();
        let handler = Handler::with_emitter_writer(Box::new(buf.clone()), Some(cm.clone()));
        let opts = Options {
            filename: Some(filename.to_owned()),
            module: Some(false),
            mode,
            ..Default::default()
        };
        // The strip pass reports unsupported syntax through the HANDLER TLS slot.
        let out = GLOBALS.set(&Globals::new(), || {
            HANDLER.set(&handler, || operate(&cm, &handler, wrapped.clone(), opts))
        });
        match out {
            Ok(out) => Ok(out.code),
            Err(e) => {
                let diag = String::from_utf8_lossy(&buf.0.lock().unwrap()).trim().to_owned();
                let msg = if diag.is_empty() { e.message.clone() } else { diag };
                Err((Some(e.code), msg.replace(PREFIX, "")))
            }
        }
    };

    match attempt(Mode::StripOnly) {
        Ok(js) => {
            // Strip-only preserves byte offsets, so the wrapper is exactly where we put it.
            Ok(js[PREFIX.len()..js.len() - SUFFIX.len()].to_owned())
        }
        Err((Some(ErrorCode::UnsupportedSyntax), _)) => {
            let js = attempt(Mode::Transform).map_err(|(_, m)| m)?;
            let open = js.find('{').ok_or("typescript transform produced unexpected output")?;
            let close = js.rfind('}').ok_or("typescript transform produced unexpected output")?;
            Ok(js[open + 1..close].to_owned())
        }
        Err((_, msg)) => Err(msg),
    }
}

#[cfg(not(feature = "typescript"))]
pub(crate) fn strip(code: &str, _filename: &str) -> Result<String, String> {
    Ok(code.to_owned())
}

/// Make a session cell's top-level `const`/`let` declarations re-declarable,
/// like Node's REPL: models re-run and tweak code, and a second
/// `const users = …` in the same session would otherwise throw
/// "redeclaration of 'users'". They become `var`, padded to the same length
/// so error line and column numbers don't move. Only top-level declarations
/// are touched (found with a real parser, so strings, comments and blocks are
/// safe); code that doesn't parse is returned unchanged.
#[cfg(feature = "typescript")]
pub(crate) fn redeclarable(js: &str) -> String {
    use swc_common::sync::Lrc;
    use swc_common::{FileName, SourceMap, Spanned};
    use swc_ecma_ast::{Decl, EsVersion, ModuleItem, Stmt, VarDeclKind};
    use swc_ecma_parser::{EsSyntax, Parser, StringInput, Syntax, lexer::Lexer};

    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), js.to_owned());
    // Parsed as a module so top-level `await` is accepted.
    let lexer = Lexer::new(Syntax::Es(EsSyntax::default()), EsVersion::latest(), StringInput::from(&*fm), None);
    let mut parser = Parser::new_from(lexer);
    let Ok(module) = parser.parse_module() else { return js.to_owned() };
    if !parser.take_errors().is_empty() {
        return js.to_owned();
    }
    let mut out = js.as_bytes().to_vec();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(decl))) = item else { continue };
        let at = (decl.span().lo.0 - fm.start_pos.0) as usize;
        let (keyword, replacement): (&[u8], &[u8]) = match decl.kind {
            VarDeclKind::Const => (b"const", b"var  "),
            VarDeclKind::Let => (b"let", b"var"),
            VarDeclKind::Var => continue,
        };
        if out.get(at..at + keyword.len()) == Some(keyword) {
            out[at..at + keyword.len()].copy_from_slice(replacement);
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| js.to_owned())
}

#[cfg(not(feature = "typescript"))]
pub(crate) fn redeclarable(js: &str) -> String {
    js.to_owned()
}

#[cfg(all(test, feature = "typescript"))]
mod tests {
    use super::strip;

    #[test]
    fn strips_types_in_place() {
        let js = strip("const x: number = 1;\nreturn x as number;", "a.ts").unwrap();
        assert_eq!(js.lines().count(), 2);
        assert!(js.contains("const x"));
        assert!(!js.contains("number"));
        // Columns preserved: `return` starts at column 0 of line 2.
        assert!(js.lines().nth(1).unwrap().starts_with("return x"));
    }

    #[test]
    fn enums_fall_back_to_transform() {
        let js = strip("enum Color { Red, Green }\nreturn Color.Green;", "a.ts").unwrap();
        assert!(js.contains("Color"));
        assert!(!js.contains("enum"));
    }

    #[test]
    fn syntax_errors_are_reported() {
        let err = strip("const = 1;", "a.ts").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn redeclarable_rewrites_only_top_level_bindings() {
        use super::redeclarable;
        let src = "const a = 1; let b = await f();\nif (x) { const inner = 2 }\nconst s = 'const kept';\nfor (let i = 0; i < 2; i++) {}";
        let out = redeclarable(src);
        assert_eq!(out, "var   a = 1; var b = await f();\nif (x) { const inner = 2 }\nvar   s = 'const kept';\nfor (let i = 0; i < 2; i++) {}");
        assert_eq!(out.len(), src.len(), "byte offsets must not move");
        // Not valid as a module (top-level return): left alone.
        assert_eq!(redeclarable("const a = 1; return a"), "const a = 1; return a");
    }

    #[test]
    fn top_level_await_and_return_parse() {
        let js = strip("const r = await tools.x();\nreturn r;", "a.ts").unwrap();
        assert!(js.contains("await tools.x()"));
    }
}
