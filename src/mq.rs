use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::collections::vec_deque::{VecDeque};
use std::marker::{Send, Sync};

pub type MessageQueueError<'a, T> = PoisonError<MutexGuard<'a, VecDeque<T>>>;

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
    pub fn send(&self, val: T) -> Result<(), MessageQueueError<'_, T>> {
        let mut q = self.queue.0.lock()?;
        q.push_back(val);
        self.queue.1.notify_one();

        Ok(())
    }

    pub fn send_or_replace(&self, new_val: T) -> Result<(), MessageQueueError<'_, T>> {
        let mut q = self.queue.0.lock()?;
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

    pub fn send_or_replace_if<F: FnOnce(&T) -> bool>(&self, pred: F, new_val: T) -> Result<(), MessageQueueError<'_, T>> {
        let mut q = self.queue.0.lock()?;
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

    pub fn is_empty(&self) -> Result<bool, MessageQueueError<'_, T>> {
        let guard = self.queue.0.lock()?;
        Ok(guard.is_empty())
    }
}

unsafe impl<T: Send> Send for MessageQueueSender<T> {}
unsafe impl<T: Send> Sync for MessageQueueSender<T> {}

impl<T> MessageQueueReceiver<T> {
    pub fn drain(&self) -> Result<Box<[T]>, MessageQueueError<'_, T>> {
        let (lock, cvar) = &*self.queue;
        let mut guard = cvar.wait_while(lock.lock()?, |vd| { vd.is_empty() })?;
        let drain = guard.drain(..).collect();
        Ok(drain)
    }

    pub fn recv(&self) -> Result<T, MessageQueueError<'_, T>> {
        let (lock, cvar) = &*self.queue;
        let mut guard = cvar.wait_while(lock.lock()?, |vd| { vd.is_empty() })?;
        Ok(guard.pop_front().unwrap())
    }
}

unsafe impl<T: Send> Send for MessageQueueReceiver<T> {}
// impl<T> !Sync for MessageQueueReceiver<T> {}
