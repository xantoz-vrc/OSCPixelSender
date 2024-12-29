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

fn main() -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mq::mq::<Message>();

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

    for _i in 0..5 {
        tx.send(Message::Clear).unwrap();
        thread::sleep(Duration::from_secs_f64(0.1));
    }

    let _handle2 = thread::spawn({
        let tx = tx.clone();
        move || {
            thread::sleep(Duration::from_secs(1));
            for _i in 0..10 {
                tx.send(Message::Clear).unwrap();
                thread::sleep(Duration::from_secs(1));
            }
        }
    });

    for i in 1..100 {
        // tx.send_or_replace(Message::Update(i)).unwrap();
        tx.send_or_replace_if(|m| *m != Message::Clear, Message::Update(i)).unwrap();
        thread::sleep(Duration::from_secs_f64(0.2));
    }

    thread::sleep(Duration::from_secs_f64(0.5));

    for i in 0..5 {
        tx.send(Message::Update(i - 100)).unwrap();
        tx.send(Message::Clear).unwrap();
        thread::sleep(Duration::from_secs_f64(0.1));
    }

    tx.send(Message::Stop).unwrap();

    println!("{}", "Main thread DONE");

    _handle2.join().map_err(|err| format!("Join fail: {err:?}"))?;
    _handle1.join().map_err(|err| format!("Join fail: {err:?}"))?;

    Ok(())
}
