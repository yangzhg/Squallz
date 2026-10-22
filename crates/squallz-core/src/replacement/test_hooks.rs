use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) enum Event {
    BeforeMove { to: PathBuf },
    AfterMove { from: PathBuf, to: PathBuf },
    BeforeSync,
}
type Events = Box<dyn FnMut(&Event) -> io::Result<()>>;
type Move = Box<dyn FnMut(&Path, &Path) -> io::Result<()>>;
type Sync = Box<dyn FnMut(&Path) -> io::Result<()>>;
enum Hook {
    Events(Events),
    Move(Move),
    Sync(Sync),
}
struct Slot {
    id: u64,
    hook: Option<Hook>,
}
thread_local! {
    static HOOKS: RefCell<(u64, Vec<Slot>)> = const { RefCell::new((0, Vec::new())) };
}
pub(crate) struct Guard(u64);
impl Drop for Guard {
    fn drop(&mut self) {
        HOOKS.with(|hooks| hooks.borrow_mut().1.retain(|slot| slot.id != self.0));
    }
}
fn push(hook: Hook) -> Guard {
    HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        hooks.0 += 1;
        let id = hooks.0;
        hooks.1.push(Slot {
            id,
            hook: Some(hook),
        });
        Guard(id)
    })
}
pub(crate) fn install(callback: Events) -> Guard {
    push(Hook::Events(callback))
}
pub(crate) fn install_move(callback: Move) -> Guard {
    push(Hook::Move(callback))
}
pub(crate) fn install_sync(callback: Sync) -> Guard {
    push(Hook::Sync(callback))
}
fn run<T>(kind: fn(&Hook) -> bool, call: impl FnOnce(&mut Hook) -> T) -> Option<T> {
    let (id, mut hook) = HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        let slot = hooks
            .1
            .iter_mut()
            .rev()
            .find(|slot| slot.hook.as_ref().is_some_and(kind))?;
        Some((slot.id, slot.hook.take()?))
    })?;
    let result = call(&mut hook);
    HOOKS.with(|hooks| {
        if let Some(slot) = hooks.borrow_mut().1.iter_mut().find(|slot| slot.id == id) {
            slot.hook = Some(hook);
        }
    });
    Some(result)
}
pub(crate) fn emit(event: Event) -> io::Result<()> {
    run(
        |hook| matches!(hook, Hook::Events(_)),
        |hook| match hook {
            Hook::Events(callback) => callback(&event),
            _ => Ok(()),
        },
    )
    .unwrap_or(Ok(()))
}
pub(crate) fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    run(
        |hook| matches!(hook, Hook::Move(_)),
        |hook| match hook {
            Hook::Move(callback) => callback(from, to),
            _ => Ok(()),
        },
    )
    .unwrap_or_else(|| crate::move_path_no_replace(from, to))
}
pub(crate) fn sync(path: &Path, file: &std::fs::File) -> io::Result<()> {
    run(
        |hook| matches!(hook, Hook::Sync(_)),
        |hook| match hook {
            Hook::Sync(callback) => callback(path),
            _ => Ok(()),
        },
    )
    .unwrap_or_else(|| file.sync_all())
}
