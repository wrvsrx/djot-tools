use std::ffi::CStr;
use std::fmt::Debug;

use tree_sitter::{ffi::ts_node_string, InputEdit, Parser};

fn main() {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_djot::language())
        .expect("Error loading djot grammer");
    let source_code = "_This is regular_ not strong emphasis\n*strong*\n";
    let mut tree = parser.parse(source_code, None).unwrap();
    unsafe {
        let cstr = CStr::from_ptr(ts_node_string(tree.root_node().into_raw()));
        println!(
            "{}",
            String::from_utf8_lossy(cstr.to_bytes()).to_string()
        );
    }
    // println!("Hello, world!");
}
