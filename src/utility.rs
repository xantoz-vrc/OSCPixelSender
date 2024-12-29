use crate::AppMessage;

use std::sync::mpsc;
use std::error::Error;

pub fn print_err<T, E: Error>(result: Result<T, E>) -> () {
    match result {
        Ok(_t) => (),
        Err(err) => eprintln!("{}", err),
    }
}

pub fn error_alert(appmsg: &mpsc::Sender<AppMessage>, message: String) -> () {
    eprintln!("{}", message);
    print_err(appmsg.send(AppMessage::Alert(message)));
    fltk::app::awake();
}
