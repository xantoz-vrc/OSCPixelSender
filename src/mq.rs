// TODO: Need to support "Disconnected" state like e.g. std::mpsc::chanel. If the sender disconnects the receiver might need to know

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::collections::vec_deque::{VecDeque};
use std::error::Error;

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
        let mut q = match self.queue.0.lock() {
            Ok(q) => q,
            Err(err) => return Err(SendError::<T> { data: val, message: format!("Error locking mutex: {err}") }),
        };

        q.push_back(val);
        self.queue.1.notify_all(); // Might only be neccessary when the queue was empty prior to push_back

        Ok(())
    }

    pub fn send_or_replace(&self, val: T) -> Result<(), SendError<T>> {
        let mut q = match self.queue.0.lock() {
            Ok(q) => q,
            Err(err) => return Err(SendError::<T> { data: val, message: format!("Error locking mutex: {err}") }),
        };

        match q.back_mut() {
            Some(x) => {
                *x = val;
            },
            None => {
                q.push_back(val);
                self.queue.1.notify_all();
            },
        }

        Ok(())
    }

    pub fn send_or_replace_if<F: FnOnce(&T) -> bool>(&self, pred: F, val: T) -> Result<(), SendError<T>> {
        let mut q = match self.queue.0.lock() {
            Ok(q) => q,
            Err(err) => return Err(SendError::<T> { data: val, message: format!("Error locking mutex: {err}") }),
        };

        match q.back_mut() {
            Some(x) => {
                if pred(x) {
                    *x = val;
                } else {
                    q.push_back(val);
                    self.queue.1.notify_all(); // Might be unneccessary since queue was already not empty
                }
            },
            None => {
                q.push_back(val);
                self.queue.1.notify_all();
            },
        }

        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool, SendError<()>> {
        let q = self.queue.0.lock()
            .map_err(|err| SendError::<()> { data: (), message: format!("Error locking mutex: {err}") })?;
        Ok(q.is_empty())
    }
}

impl<T> MessageQueueReceiver<T> {
    fn wait_until_nonempty(&self) -> Result<MutexGuard<'_, VecDeque<T>>, RecvError> {
        let (lock, cvar) = &*self.queue;
        let guard = cvar.wait_while(
            lock.lock()
                .map_err(|err| RecvError{ message: format!("Error locking mutex: {err}") })?,
            |vd| { vd.is_empty() },
        ).map_err(|err| RecvError{ message: format!("Error waiting on Condvar: {err}") })?;
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

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut q = self.queue.0.lock()
            .map_err(|err| TryRecvError::RecvError(RecvError{ message: format!("Error locking mutex: {err}") }))?;
        if q.is_empty() {
            Err(TryRecvError::Empty)
        } else {
            Ok(q.pop_front().unwrap())
        }
    }
}

// ERROR HANDLING
pub struct SendError<T> {
    pub data: T,
    pub message: String,
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
    pub message: String,
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for RecvError {}

#[derive(Debug)]
pub enum TryRecvError {
    RecvError(RecvError),
    Empty,
}

