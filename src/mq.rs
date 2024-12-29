use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::collections::vec_deque::{VecDeque};
use std::error::Error;

pub struct SendError<T> {
    pub data: T,
    pub message: &'static str,
}

impl<T> std::fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SendError<{}> {{ data: .., message: {:?} }}", std::any::type_name::<T>(), self.message)
    }
}

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl<T> Error for SendError<T> {}

#[derive(Debug)]
pub struct RecvError {
    pub message: &'static str,
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for RecvError {}

#[derive(Debug, Clone)]
pub struct MessageQueueSender<T> {
    queue: Arc<(Mutex<VecDeque<T>>, Condvar)>,
}

#[derive(Debug)]
pub struct MessageQueueReceiver<T> {
    queue: Arc<(Mutex<VecDeque<T>>, Condvar)>,
}

pub fn mq<T>() -> (MessageQueueSender<T>, MessageQueueReceiver<T>) {
    let q = Arc::new((Mutex::new(VecDeque::<T>::new()), Condvar::new()));
    let q2 = Arc::clone(&q);

    (MessageQueueSender::<T> { queue: q }, MessageQueueReceiver::<T> { queue: q2 })
}

impl<T> MessageQueueSender<T> {
    pub fn send(&self, val: T) -> Result<(), SendError<T>> {
        let lockres = self.queue.0.lock();
        let Ok(mut q) = lockres else {
            return Err(SendError::<T> { data: val, message: "Error locking mutex", });
        };

        q.push_back(val);
        self.queue.1.notify_one();

        Ok(())
    }

    pub fn send_or_replace(&self, new_val: T) -> Result<(), SendError<T>> {
        let lockres = self.queue.0.lock();
        let Ok(mut q) = lockres else {
            return Err(SendError::<T> { data: new_val, message: "Error locking mutex", });
        };

        match q.back_mut() {
            Some(x) => {
                *x = new_val;
            },
            None => {
                q.push_back(new_val);
                self.queue.1.notify_one();
            },
        }

        Ok(())
    }

    pub fn send_or_replace_if<F: FnOnce(&T) -> bool>(&self, pred: F, new_val: T) -> Result<(), SendError<T>> {
        let lockres = self.queue.0.lock();
        let Ok(mut q) = lockres else {
            return Err(SendError::<T> { data: new_val, message: "Error locking mutex", });
        };

        match q.back_mut() {
            Some(x) => {
                if pred(x) {
                    *x = new_val;
                }
            },
            None => {
                q.push_back(new_val);
                self.queue.1.notify_one();
            },
        }

        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool, SendError<()>> {
        let lockres = self.queue.0.lock();
        let Ok(q) = lockres else {
            return Err(SendError::<_> { data: (), message: "Error locking mutex", });
        };

        Ok(q.is_empty())
    }
}

impl<T> MessageQueueReceiver<T> {
    fn wait_until_nonempty(&self) -> Result<MutexGuard<'_, VecDeque<T>>, RecvError> {
        let (lock, cvar) = &*self.queue;
        let guard = cvar.wait_while(
            lock.lock()
                .map_err(|_err| RecvError{ message: "Error locking mutex" })?,
            |vd| { vd.is_empty() },
        ).map_err(|_err| RecvError{ message: "Error waiting on Condvar" })?;
        Ok(guard)
    }

    pub fn drain(&self) -> Result<Box<[T]>, RecvError> {
        let mut guard = self.wait_until_nonempty()?;
        let drain = guard.drain(..).collect();
        Ok(drain)
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        let mut guard = self.wait_until_nonempty()?;
        Ok(guard.pop_front().unwrap())
    }
}
