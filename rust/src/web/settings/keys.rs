//! Logger and explicit HTTP-boundary configuration; credential routes arrive in Task 3.
use std::sync::Arc;
pub trait Logger:Send+Sync{fn warn(&self,line:&str);}
struct Stderr;impl Logger for Stderr{fn warn(&self,line:&str){eprintln!("{line}");}}
pub fn logger()->Arc<dyn Logger>{Arc::new(Stderr)}
