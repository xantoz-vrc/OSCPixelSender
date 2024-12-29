use std::error::Error;

pub fn print_err<T, E: Error>(result: Result<T, E>) -> () {
    match result {
        Ok(_t) => (),
        Err(err) => eprintln!("{}", err),
    }
}
