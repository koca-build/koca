use koca::Cli;
use std::{io::{self, Cursor}, process};
use husk::syntax::Parser;

fn main() {
    let mut cursor = Cursor::new("hi() {");
    let parser = Parser::builder().build();
    println!("{}", parser.parse(&mut cursor, Some("wow.txt")).unwrap_err());
    return;

    let exit_code = match Cli::run() {
        Ok(code) => code,
        Err(err) => err.exit(),
    };
    process::exit(exit_code);
}
