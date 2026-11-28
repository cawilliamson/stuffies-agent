//! steps more than one capability names, defined once so the match is unambiguous.

use cucumber::then;

use crate::world::Spec;

#[then("it is refused with `-32602`")]
fn it_is_refused_as_invalid(world: &mut Spec) {
    assert_eq!(world.code(), -32602, "{:?}", world.response);
}

#[then("it is refused with `-32005`")]
fn it_is_refused_as_missing(world: &mut Spec) {
    assert_eq!(world.code(), -32005, "{:?}", world.response);
}

#[then("it is refused with `-32004`")]
fn it_is_refused_as_busy(world: &mut Spec) {
    assert_eq!(world.code(), -32004, "{:?}", world.response);
}
