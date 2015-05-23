// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#![feature(rustc_private)]
#![feature(collections)]
#![feature(str_char)]

// TODO we're going to allocate a whole bunch of temp Strings, is it worth
// keeping some scratch mem for this and running our own StrPool?
// TODO for lint violations of names, emit a refactor script


#[macro_use]
extern crate log;

extern crate getopts;
extern crate rustc;
extern crate rustc_driver;
extern crate syntax;
extern crate rustc_serialize;

extern crate strings;

use rustc::session::Session;
use rustc::session::config as rustc_config;
use rustc::session::config::Input;
use rustc_driver::{driver, CompilerCalls, Compilation};

use rustc_serialize::{Decodable, Decoder};

use syntax::ast;
use syntax::codemap::CodeMap;
use syntax::diagnostics;
use syntax::visit;

use std::path::PathBuf;
use std::collections::HashMap;

use changes::ChangeSet;
use visitor::FmtVisitor;

#[macro_use]
mod config;
mod changes;
mod visitor;
mod functions;
mod missed_spans;
mod lists;
mod utils;
mod types;
mod expr;
mod imports;

const MIN_STRING: usize = 10;
// When we get scoped annotations, we should have rustfmt::skip.
const SKIP_ANNOTATION: &'static str = "rustfmt_skip";

static mut CONFIG: Option<config::Config> = None;

#[derive(Copy, Clone)]
pub enum WriteMode {
    Overwrite,
    // str is the extension of the new file
    NewFile(&'static str),
    // Write the output to stdout.
    Display,
    // Return the result as a mapping from filenames to StringBuffers.
    Return(&'static Fn(HashMap<String, String>)),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum NewlineStyle {
    Windows, // \r\n
    Unix, // \n
}

impl Decodable for NewlineStyle {
    fn decode<D: Decoder>(d: &mut D) -> Result<Self, D::Error> {
        let s = try!(d.read_str());
        match &*s {
            "Windows" => Ok(NewlineStyle::Windows),
            "Unix" => Ok(NewlineStyle::Unix),
            _ => Err(d.error("Bad variant")),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BraceStyle {
    AlwaysNextLine,
    PreferSameLine,
    // Prefer same line except where there is a where clause, in which case force
    // the brace to the next line.
    SameLineWhere,
}

impl Decodable for BraceStyle {
    fn decode<D: Decoder>(d: &mut D) -> Result<Self, D::Error> {
        let s = try!(d.read_str());
        match &*s {
            "AlwaysNextLine" => Ok(BraceStyle::AlwaysNextLine),
            "PreferSameLine" => Ok(BraceStyle::PreferSameLine),
            "SameLineWhere" => Ok(BraceStyle::SameLineWhere),
            _ => Err(d.error("Bad variant")),
        }
    }
}

// How to indent a function's return type.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReturnIndent {
    // Aligned with the arguments
    WithArgs,
    // Aligned with the where clause
    WithWhereClause,
}

// TODO could use a macro for all these Decodable impls.
impl Decodable for ReturnIndent {
    fn decode<D: Decoder>(d: &mut D) -> Result<Self, D::Error> {
        let s = try!(d.read_str());
        match &*s {
            "WithArgs" => Ok(ReturnIndent::WithArgs),
            "WithWhereClause" => Ok(ReturnIndent::WithWhereClause),
            _ => Err(d.error("Bad variant")),
        }
    }
}

// Formatting which depends on the AST.
fn fmt_ast<'a>(krate: &ast::Crate, codemap: &'a CodeMap) -> ChangeSet<'a> {
    let mut visitor = FmtVisitor::from_codemap(codemap);
    visit::walk_crate(&mut visitor, krate);
    let files = codemap.files.borrow();
    if let Some(last) = files.last() {
        visitor.format_missing(last.end_pos);
    }

    visitor.changes
}

// Formatting done on a char by char or line by line basis.
// TODO warn on TODOs and FIXMEs without an issue number
// TODO warn on bad license
// TODO other stuff for parity with make tidy
fn fmt_lines(changes: &mut ChangeSet) {
    let mut truncate_todo = Vec::new();

    // Iterate over the chars in the change set.
    for (f, text) in changes.text() {
        let mut trims = vec![];
        let mut last_wspace: Option<usize> = None;
        let mut line_len = 0;
        let mut cur_line = 1;
        let mut newline_count = 0;
        for (c, b) in text.chars() {
            if c == '\r' { continue; }
            if c == '\n' {
                // Check for (and record) trailing whitespace.
                if let Some(lw) = last_wspace {
                    trims.push((cur_line, lw, b));
                    line_len -= b - lw;
                }
                // Check for any line width errors we couldn't correct.
                if line_len > config!(max_width) {
                    // TODO store the error rather than reporting immediately.
                    println!("Rustfmt couldn't fix (sorry). {}:{}: line longer than {} characters",
                             f, cur_line, config!(max_width));
                }
                line_len = 0;
                cur_line += 1;
                newline_count += 1;
                last_wspace = None;
            } else {
                newline_count = 0;
                line_len += 1;
                if c.is_whitespace() {
                    if last_wspace.is_none() {
                        last_wspace = Some(b);
                    }
                } else {
                    last_wspace = None;
                }
            }
        }

        if newline_count > 1 {
            debug!("track truncate: {} {} {}", f, text.len, newline_count);
            truncate_todo.push((f.to_string(), text.len - newline_count + 1))
        }

        for &(l, _, _) in trims.iter() {
            // TODO store the error rather than reporting immediately.
            println!("Rustfmt left trailing whitespace at {}:{} (sorry)", f, l);
        }
    }

    for (f, l) in truncate_todo {
        changes.get_mut(&f).truncate(l);
    }
}

struct RustFmtCalls {
    input_path: Option<PathBuf>,
    write_mode: WriteMode,
}

impl<'a> CompilerCalls<'a> for RustFmtCalls {
    fn early_callback(&mut self,
                      _: &getopts::Matches,
                      _: &diagnostics::registry::Registry)
                      -> Compilation {
        Compilation::Continue
    }

    fn some_input(&mut self,
                  input: Input,
                  input_path: Option<PathBuf>)
                  -> (Input, Option<PathBuf>) {
        match input_path {
            Some(ref ip) => self.input_path = Some(ip.clone()),
            _ => {
                // FIXME should handle string input and write to stdout or something
                panic!("No input path");
            }
        }
        (input, input_path)
    }

    fn no_input(&mut self,
                _: &getopts::Matches,
                _: &rustc_config::Options,
                _: &Option<PathBuf>,
                _: &Option<PathBuf>,
                _: &diagnostics::registry::Registry)
                -> Option<(Input, Option<PathBuf>)> {
        panic!("No input supplied to RustFmt");
    }

    fn late_callback(&mut self,
                     _: &getopts::Matches,
                     _: &Session,
                     _: &Input,
                     _: &Option<PathBuf>,
                     _: &Option<PathBuf>)
                     -> Compilation {
        Compilation::Continue
    }

    fn build_controller(&mut self, _: &Session) -> driver::CompileController<'a> {
        let write_mode = self.write_mode;
        let mut control = driver::CompileController::basic();
        control.after_parse.stop = Compilation::Stop;
        control.after_parse.callback = Box::new(move |state| {
            let krate = state.krate.unwrap();
            let codemap = state.session.codemap();
            let mut changes = fmt_ast(krate, codemap);
            // For some reason, the codemap does not include terminating newlines
            // so we must add one on for each file. This is sad.
            changes.append_newlines();
            fmt_lines(&mut changes);

            // FIXME(#5) Should be user specified whether to show or replace.
            let result = changes.write_all_files(write_mode);

            match result {
                Err(msg) => println!("Error writing files: {}", msg),
                Ok(result) => {
                    if let WriteMode::Return(callback) = write_mode {
                        callback(result);
                    }
                }
            }
        });

        control
    }
}

// args are the arguments passed on the command line, generally passed through
// to the compiler.
// write_mode determines what happens to the result of running rustfmt, see
// WriteMode.
// default_config is a string of toml data to be used to configure rustfmt.
pub fn run(args: Vec<String>, write_mode: WriteMode, default_config: &str) {
    config::set_config(default_config);

    let mut call_ctxt = RustFmtCalls { input_path: None, write_mode: write_mode };
    rustc_driver::run_compiler(&args, &mut call_ctxt);
}
