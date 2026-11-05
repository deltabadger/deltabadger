//! `deltabadger reset-password`'s input (PLAN-A16b §1, §1a): the new password from the terminal (asked twice, nothing
//! echoed), from a pipe (`--stdin`), or made here (`--generate`). Never from an argument.
//!
//! On the terminal, fd 0 goes to non-canonical mode with ECHO and ISIG off, so the keyboard sends no signals: the
//! terminal's interrupt, quit and suspend characters (Ctrl-C, Ctrl-\, Ctrl-Z) are read as bytes and cancel. Nothing here
//! exits while the terminal is changed; the settings go back with TCSAFLUSH (which drops anything typed but unread)
//! before any answer is returned. A signal sent from outside meanwhile (kill) restores them in its handler and exits 130.

/// The longest line read, in bytes. The policy's own bound is 128 characters (web::password).
pub const MAX_BYTES: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Interrupt, quit or suspend typed, or a signal: exit 130.
    Cancelled,
    NoPassword,
    TooLong,
    NotText,
    Mismatch,
    /// The terminal could not be read or set up.
    Terminal(String),
}

impl Refused {
    pub fn message(&self) -> String {
        match self {
            Refused::Cancelled => "cancelled; nothing was changed".into(),
            Refused::NoPassword => "no password given; nothing was changed".into(),
            Refused::TooLong => format!("the password is too long (more than {MAX_BYTES} bytes); nothing was changed"),
            Refused::NotText => "the password is not valid UTF-8 text; nothing was changed".into(),
            Refused::Mismatch => "the passwords didn't match; nothing was changed".into(),
            Refused::Terminal(e) => format!("cannot read the terminal ({e}); nothing was changed"),
        }
    }
}

/// One line from `input`: up to `\n` or the end, its `\r\n`/`\n` dropped, every other byte kept. Nothing after the `\n`
/// is read, so a script that keeps its pipe open isn't waited on.
pub fn read_line(input: &mut impl std::io::Read) -> Result<String, Refused> {
    let mut line = Vec::new();
    let (mut byte, mut ended) = ([0u8; 1], false);
    // One byte past the bound (and its \r) tells a line at the bound from a longer one.
    while line.len() <= MAX_BYTES + 1 {
        match input.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => { ended = true; break }
            Ok(_) => line.push(byte[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Refused::Terminal(e.to_string())),
        }
    }
    if ended && line.last() == Some(&b'\r') { line.pop(); } // CRLF; a lone \r at the end of the input is the password's
    if line.len() > MAX_BYTES { return Err(Refused::TooLong) }
    if line.is_empty() { return Err(Refused::NoPassword) }
    String::from_utf8(line).map_err(|_| Refused::NotText)
}

/// A password the web form accepts: Devise friendly tokens joined by `-` (so it has a symbol), drawn again until it has
/// a lowercase and an uppercase letter and a digit too.
pub fn generate() -> String {
    loop {
        let password = format!("{}-{}", crate::crypto::friendly_token(), crate::crypto::friendly_token());
        if crate::web::password::policy_message(&password).is_none() { return password }
    }
}

/// The new password, typed twice on the terminal on fd 0, prompts on stderr.
#[cfg(unix)]
pub fn prompt_twice(label: &str) -> Result<String, Refused> {
    let raw = terminal::Raw::enter().map_err(Refused::Terminal)?;
    let first = raw.line(label);
    let second = first.is_ok().then(|| raw.line("Again: "));
    raw.restore().map_err(Refused::Terminal)?; // the terminal back as it was, before anything is answered
    let first = first?;
    if second.expect("asked after a first")? != first { return Err(Refused::Mismatch) }
    Ok(first)
}

#[cfg(not(unix))]
pub fn prompt_twice(_label: &str) -> Result<String, Refused> {
    Err(Refused::Terminal("no terminal prompt on this system: use --stdin or --generate".into()))
}

#[cfg(unix)]
mod terminal {
    use super::{Refused, MAX_BYTES};
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    /// The settings to restore, for the signal handler too (read with an atomic flag, set before the handler is armed).
    static SAVED: OnceLock<libc::termios> = OnceLock::new();
    static CHANGED: AtomicBool = AtomicBool::new(false);
    const SIGNALS: [libc::c_int; 5] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT, libc::SIGTSTP];

    extern "C" fn restore_and_exit(_: libc::c_int) {
        // tcsetattr, write and _exit are async-signal-safe.
        if CHANGED.load(Ordering::SeqCst) {
            if let Some(saved) = SAVED.get() { let _ = set(saved); }
        }
        unsafe { libc::write(2, b"\n".as_ptr().cast(), 1); libc::_exit(130) }
    }

    pub struct Raw { saved: libc::termios, previous: Vec<(libc::c_int, libc::sigaction)>, restored: bool }

    /// tcsetattr(TCSAFLUSH), again when a signal interrupts it. Allocation-free, so the signal handler can call it; the
    /// error is formatted by the caller, outside signal context.
    fn set(t: &libc::termios) -> std::io::Result<()> {
        loop {
            if unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, t) } == 0 { return Ok(()) }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted { return Err(e) }
        }
    }

    impl Raw {
        pub fn enter() -> Result<Raw, String> {
            let mut saved: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(0, &mut saved) } != 0 { return Err(std::io::Error::last_os_error().to_string()) }
            let _ = SAVED.set(saved); // one terminal per process: the first reading is the one to restore
            let mut previous = Vec::new();
            for signal in SIGNALS {
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = restore_and_exit as extern "C" fn(libc::c_int) as usize;
                let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
                if unsafe { libc::sigaction(signal, &action, &mut old) } == 0 { previous.push((signal, old)) }
            }
            let mut raw = saved;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            let entered = Raw { saved, previous, restored: false };
            CHANGED.store(true, Ordering::SeqCst);
            set(&raw).map_err(|e| e.to_string())?; // on failure, Drop restores
            Ok(entered)
        }

        /// One line typed after `label`: Enter ends it, erase rubs out a character, interrupt/quit/suspend cancel, end of
        /// file on an empty line gives none.
        pub fn line(&self, label: &str) -> Result<String, Refused> {
            let mut err = std::io::stderr();
            let _ = write!(err, "{label}").and_then(|_| err.flush());
            let cc = |i: usize| self.saved.c_cc[i];
            let mut line: Vec<u8> = Vec::new();
            let result = loop {
                let mut byte = 0u8;
                let n = unsafe { libc::read(0, (&mut byte as *mut u8).cast(), 1) };
                if n < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::Interrupted { continue }
                    break Err(Refused::Terminal(e.to_string()));
                }
                match byte {
                    _ if n == 0 => break Err(Refused::NoPassword), // the terminal hung up
                    b'\r' | b'\n' => break if line.is_empty() { Err(Refused::NoPassword) } else { String::from_utf8(std::mem::take(&mut line)).map_err(|_| Refused::NotText) },
                    b if b == cc(libc::VINTR) || b == cc(libc::VQUIT) || b == cc(libc::VSUSP) => break Err(Refused::Cancelled),
                    b if b == cc(libc::VEOF) => if line.is_empty() { break Err(Refused::NoPassword) },
                    b if b == cc(libc::VERASE) || b == 0x08 || b == 0x7f => {
                        // A whole UTF-8 character: its continuation bytes, then its first.
                        while line.last().is_some_and(|b| b & 0xC0 == 0x80) { line.pop(); }
                        line.pop();
                    }
                    b => {
                        if line.len() == MAX_BYTES { break Err(Refused::TooLong) }
                        line.push(b);
                    }
                }
            };
            let _ = writeln!(err);
            line.fill(0);
            result
        }

        /// The terminal as it was, checked: a failure is the caller's to report, before anything is written. The signal
        /// handlers stay armed until the settings are back.
        pub fn restore(mut self) -> Result<(), String> {
            set(&self.saved).map_err(|e| e.to_string())?;
            self.restored = true;
            Ok(())
        }
    }

    impl Drop for Raw {
        fn drop(&mut self) {
            if !self.restored && set(&self.saved).is_err() { return } // the handlers stay armed: a later signal tries again
            CHANGED.store(false, Ordering::SeqCst);
            for (signal, old) in &self.previous { unsafe { libc::sigaction(*signal, old, std::ptr::null_mut()) }; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_keeps_its_spaces_and_drops_only_its_ending() {
        assert_eq!(read_line(&mut &b" two words-A1 \r\nrest"[..]), Ok(" two words-A1 ".into()));
        assert_eq!(read_line(&mut &b"no-newline-A1"[..]), Ok("no-newline-A1".into()));
        assert_eq!(read_line(&mut &b"ends-in-cr-A1\r"[..]), Ok("ends-in-cr-A1\r".into()), "a lone \\r at the end is kept");
        assert_eq!(read_line(&mut &b"\n"[..]), Err(Refused::NoPassword));
        assert_eq!(read_line(&mut &b""[..]), Err(Refused::NoPassword));
        assert_eq!(read_line(&mut &b"\xff\xfe-A1a\n"[..]), Err(Refused::NotText));
        assert_eq!(read_line(&mut &vec![b'a'; MAX_BYTES][..]).map(|s| s.len()), Ok(MAX_BYTES));
        assert_eq!(read_line(&mut &vec![b'a'; MAX_BYTES + 1][..]), Err(Refused::TooLong));
    }

    #[test]
    fn a_generated_password_meets_the_web_forms_rules() {
        for _ in 0..1000 {
            let password = generate();
            assert!(password.len() >= 41 && crate::web::password::policy_message(&password).is_none(), "{password}");
        }
    }
}
