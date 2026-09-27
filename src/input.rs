use std::time::Duration;

pub enum Key {
    Char(char),
    Left,
    Right,
    Esc,
    CtrlC,
}

pub enum Input {
    Keys(Vec<Key>),
    Timeout,
    Closed,
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub fn wait(timeout: Duration) -> Input {
    let mut pfd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let r = unsafe { libc::poll(&mut pfd, 1, ms) };
    if r < 0 {
        return if errno() == libc::EINTR {
            Input::Timeout
        } else {
            Input::Closed
        };
    }
    if r == 0 {
        return Input::Timeout;
    }
    if pfd.revents & libc::POLLIN == 0 {
        return Input::Closed;
    }
    let mut buf = [0u8; 64];
    let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        let e = errno();
        return if e == libc::EINTR || e == libc::EAGAIN {
            Input::Timeout
        } else {
            Input::Closed
        };
    }
    if n == 0 {
        return Input::Closed;
    }
    Input::Keys(parse(&buf[..n as usize]))
}

fn parse(bytes: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x03' => keys.push(Key::CtrlC),
            '\x1b' => match chars.peek() {
                Some('[') | Some('O') => {
                    chars.next();
                    let mut last = None;
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            last = Some(c);
                            break;
                        }
                    }
                    match last {
                        Some('C') => keys.push(Key::Right),
                        Some('D') => keys.push(Key::Left),
                        _ => {}
                    }
                }
                _ => keys.push(Key::Esc),
            },
            c => keys.push(Key::Char(c)),
        }
    }
    keys
}
