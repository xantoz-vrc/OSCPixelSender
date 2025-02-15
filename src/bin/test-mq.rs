use rust_image_fiddler::mq;

use std::thread;
use std::time::Duration;
use std::error::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Update(i32),
    Clear,
    Stop,
}

impl Message {
    fn is_update(&self) -> bool {
        match self {
            Self::Update(_) => true,
            _ => false,
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mq::mq::<Message>();

/*
    let _handle1 = thread::spawn({
        move || -> () {
            let mut clear_count: i32 = 0;
            let mut run: bool = true;

            while run {
                let boxed_slice: Box<[Message]> = rx.drain().unwrap();
                dbg!(&boxed_slice);
                for msg in boxed_slice {
                    match msg {
                        Message::Update(n) => {
                            println!("Processing update #{n}");
                            // thread::sleep(Duration::from_secs_f64((n as f64)/10.0));
                            thread::sleep(Duration::from_secs(2));
                        },
                        Message::Clear => {
                            clear_count += 1;
                            println!("Clear #{}!", clear_count);
                            thread::sleep(Duration::from_secs_f64(0.4));
                        },
                        Message::Stop => {
                            println!("Got stop message. Stopping thread.");
                            run = false;
                        },
                    }
                }
            }
        }
    });
*/

    let _handle1 = thread::spawn({
        move || -> () {
            let mut clear_count: i32 = 0;
            let mut run: bool = true;

            while run {
                let msg = rx.recv().unwrap();
                match msg {
                    Message::Update(n) => {
                        println!("Processing update #{n}");
                        // thread::sleep(Duration::from_secs_f64((n as f64)/10.0));
                        thread::sleep(Duration::from_secs(2));
                        println!("Finished update #{n}");
                    },
                    Message::Clear => {
                        clear_count += 1;
                        println!("Clear #{}!", clear_count);
                        thread::sleep(Duration::from_secs_f64(0.4));
                        println!("Finished clear #{}!", clear_count);
                    },
                    Message::Stop => {
                        println!("Got stop message. Stopping thread.");
                        run = false;
                    },
                }
            }
        }
    });

    for _i in 0..5 {
        tx.send(Message::Clear)?;
        thread::sleep(Duration::from_secs_f64(0.1));
    }

    let _handle2 = thread::spawn({
        let tx = tx.clone();
        move || {
            thread::sleep(Duration::from_secs(1));
            for _i in 0..5 {
                println!("Send clear {}", _i+5);
                tx.send(Message::Clear).unwrap();
                thread::sleep(Duration::from_secs(1));
            }
            println!("Clear thread done");
        }
    });

    for i in 1..100 {
        println!("put Update {i}");
        tx.send_or_replace_if(Message::is_update, Message::Update(i))?;
        thread::sleep(Duration::from_secs_f64(0.2));
    }

    thread::sleep(Duration::from_secs_f64(0.5));

    for i in 0..5 {
        tx.send(Message::Update(i - 100))?;
        tx.send(Message::Clear)?;
        thread::sleep(Duration::from_secs_f64(0.1));
    }

    tx.send(Message::Stop)?;

    println!("{}", "Main thread DONE");

    _handle2.join().map_err(|err| format!("Join fail: {err:?}"))?;
    _handle1.join().map_err(|err| format!("Join fail: {err:?}"))?;

    println!("{}", "All threads joined");

    Ok(())
}
