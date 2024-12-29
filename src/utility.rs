use crate::AppMessage;

use std::sync::mpsc;
use std::error::Error;

pub fn print_err<T, E: Error>(result: Result<T, E>) -> () {
    match result {
        Ok(_t) => (),
        Err(err) => eprintln!("{}", err),
    }
}

pub fn alert(appmsg: &mpsc::Sender<AppMessage>, message: String) -> () {
    println!("{}", message);
    print_err(appmsg.send(AppMessage::Alert(message)));
    fltk::app::awake();
}

pub fn error_alert(appmsg: &mpsc::Sender<AppMessage>, message: String) -> () {
    eprintln!("{}", message);
    print_err(appmsg.send(AppMessage::Alert(message)));
    fltk::app::awake();
}

#[macro_export]
macro_rules! static_assert {
    ($($tt:tt)*) => {
        const _: () = assert!($($tt)*);
    }
}

#[allow(dead_code)]
pub fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>());
}
