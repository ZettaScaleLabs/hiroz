use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use parking_lot::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockKind {
    System,
    Simulated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZDuration(Duration);

impl ZDuration {
    pub fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs))
    }

    pub fn from_millis(millis: u64) -> Self {
        Self(Duration::from_millis(millis))
    }

    pub fn as_std(self) -> Duration {
        self.0
    }
}

impl Default for ZDuration {
    fn default() -> Self {
        Self(Duration::ZERO)
    }
}

impl From<Duration> for ZDuration {
    fn from(value: Duration) -> Self {
        Self(value)
    }
}

impl From<ZDuration> for Duration {
    fn from(value: ZDuration) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZTime {
    since_epoch: Duration,
}

impl fmt::Debug for ZTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZTime")
            .field("secs", &self.since_epoch.as_secs())
            .field("nanos", &self.since_epoch.subsec_nanos())
            .finish()
    }
}

impl ZTime {
    pub fn zero() -> Self {
        Self {
            since_epoch: Duration::ZERO,
        }
    }

    pub fn from_system_time(time: SystemTime) -> Self {
        let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        Self { since_epoch }
    }

    pub fn from_unix_nanos(nanos: i64) -> Self {
        let nanos = u64::try_from(nanos).unwrap_or_default();
        Self {
            since_epoch: Duration::from_nanos(nanos),
        }
    }

    pub fn to_system_time(self) -> SystemTime {
        UNIX_EPOCH + self.since_epoch
    }

    pub fn as_unix_nanos(self) -> i64 {
        self.since_epoch.as_nanos().min(i64::MAX as u128) as i64
    }

    pub fn saturating_add(self, duration: ZDuration) -> Self {
        Self {
            since_epoch: self.since_epoch.saturating_add(duration.0),
        }
    }

    pub fn saturating_sub(self, duration: ZDuration) -> Self {
        Self {
            since_epoch: self.since_epoch.saturating_sub(duration.0),
        }
    }

    pub fn duration_since(self, earlier: ZTime) -> ZDuration {
        ZDuration(self.since_epoch.saturating_sub(earlier.since_epoch))
    }
}

impl Default for ZTime {
    fn default() -> Self {
        Self::zero()
    }
}

#[derive(Debug)]
pub enum ClockError {
    NotSimulated,
    TimeWentBackwards,
}

impl fmt::Display for ClockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClockError::NotSimulated => write!(f, "clock is not simulated"),
            ClockError::TimeWentBackwards => write!(f, "simulated time cannot move backwards"),
        }
    }
}

impl std::error::Error for ClockError {}

#[derive(Clone)]
pub struct ZClock {
    inner: Arc<ClockInner>,
}

enum ClockInner {
    System,
    Simulated(SimulatedClockState),
}

struct SimulatedClockState {
    now: Mutex<ZTime>,
    notify: Notify,
}

impl fmt::Debug for ZClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZClock")
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

impl Default for ZClock {
    fn default() -> Self {
        Self::system()
    }
}

impl ZClock {
    pub fn system() -> Self {
        Self {
            inner: Arc::new(ClockInner::System),
        }
    }

    pub fn simulated(start: ZTime) -> Self {
        Self {
            inner: Arc::new(ClockInner::Simulated(SimulatedClockState {
                now: Mutex::new(start),
                notify: Notify::new(),
            })),
        }
    }

    pub fn from_kind(kind: ClockKind) -> Self {
        match kind {
            ClockKind::System => Self::system(),
            ClockKind::Simulated => Self::simulated(ZTime::zero()),
        }
    }

    pub fn kind(&self) -> ClockKind {
        match self.inner.as_ref() {
            ClockInner::System => ClockKind::System,
            ClockInner::Simulated(_) => ClockKind::Simulated,
        }
    }

    pub fn now(&self) -> ZTime {
        match self.inner.as_ref() {
            ClockInner::System => ZTime::from_system_time(SystemTime::now()),
            ClockInner::Simulated(state) => *state.now.lock(),
        }
    }

    pub fn set_time(&self, time: ZTime) -> Result<(), ClockError> {
        match self.inner.as_ref() {
            ClockInner::System => Err(ClockError::NotSimulated),
            ClockInner::Simulated(state) => {
                let mut current = state.now.lock();
                if time < *current {
                    return Err(ClockError::TimeWentBackwards);
                }
                *current = time;
                state.notify.notify_waiters();
                Ok(())
            }
        }
    }

    pub fn advance(&self, delta: ZDuration) -> Result<ZTime, ClockError> {
        match self.inner.as_ref() {
            ClockInner::System => Err(ClockError::NotSimulated),
            ClockInner::Simulated(state) => {
                let mut current = state.now.lock();
                *current = current.saturating_add(delta);
                let now = *current;
                state.notify.notify_waiters();
                Ok(now)
            }
        }
    }

    pub fn sleep_until(&self, deadline: ZTime) -> ZSleep {
        match self.inner.as_ref() {
            ClockInner::System => {
                let now = SystemTime::now();
                let deadline = deadline.to_system_time();
                let duration = deadline.duration_since(now).unwrap_or(Duration::ZERO);
                ZSleep(Box::pin(tokio::time::sleep(duration)))
            }
            ClockInner::Simulated(_) => {
                let clock = self.clone();
                ZSleep(Box::pin(async move {
                    loop {
                        // Obtain and *enable* the Notified future before checking the
                        // condition.  `enable()` registers this task as a waiter
                        // immediately, so a concurrent `notify_waiters()` call that
                        // fires between the condition check and the first `.await` poll
                        // is not lost.
                        let notified = match clock.inner.as_ref() {
                            ClockInner::System => unreachable!(),
                            ClockInner::Simulated(state) => state.notify.notified(),
                        };
                        tokio::pin!(notified);
                        notified.as_mut().enable();

                        if clock.now() >= deadline {
                            break;
                        }
                        notified.await;
                    }
                }))
            }
        }
    }

    pub fn sleep(&self, duration: impl Into<ZDuration>) -> ZSleep {
        let deadline = self.now().saturating_add(duration.into());
        self.sleep_until(deadline)
    }

    pub fn interval(&self, period: impl Into<ZDuration>) -> ZInterval {
        let period = period.into();
        ZInterval {
            clock: self.clone(),
            period,
            next_deadline: self.now().saturating_add(period),
        }
    }
}

pub struct ZSleep(Pin<Box<dyn Future<Output = ()> + Send>>);

impl Future for ZSleep {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}

pub struct ZInterval {
    clock: ZClock,
    period: ZDuration,
    next_deadline: ZTime,
}

impl ZInterval {
    pub async fn tick(&mut self) -> ZTime {
        self.clock.sleep_until(self.next_deadline).await;
        let fired_at = self.next_deadline;
        self.next_deadline = self.next_deadline.saturating_add(self.period);
        fired_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_is_default() {
        let clock = ZClock::default();
        assert_eq!(clock.kind(), ClockKind::System);
    }

    #[tokio::test]
    async fn simulated_clock_can_advance_manually() {
        let clock = ZClock::simulated(ZTime::zero());
        let mut interval = clock.interval(ZDuration::from_secs(1));

        let waiter = tokio::spawn(async move { interval.tick().await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        clock.advance(ZDuration::from_secs(1)).unwrap();
        let tick = waiter.await.unwrap();
        assert_eq!(tick, ZTime::from_unix_nanos(1_000_000_000));
    }

    #[tokio::test]
    async fn simulated_timer_follows_simulated_time() {
        let clock = ZClock::simulated(ZTime::zero());
        let sleep = clock.sleep(ZDuration::from_millis(10));

        let waiter = tokio::spawn(sleep);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        clock.advance(ZDuration::from_millis(10)).unwrap();
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn simulated_sleep_no_lost_wakeup_when_advance_before_poll() {
        // Regression test: advance the clock past the deadline *before* the sleep
        // future is ever polled.  Without the enable() fix this would hang forever
        // because notify_waiters() fires before the future registers as a waiter.
        let clock = ZClock::simulated(ZTime::zero());
        let sleep = clock.sleep(ZDuration::from_millis(10));
        // Advance BEFORE yielding — the future has not been polled yet.
        clock.advance(ZDuration::from_millis(10)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), sleep)
            .await
            .expect("sleep should resolve without hanging");
    }

    // --- ZDuration ---

    #[test]
    fn zduration_from_secs_and_as_std() {
        let d = ZDuration::from_secs(3);
        assert_eq!(d.as_std(), Duration::from_secs(3));
    }

    #[test]
    fn zduration_default_is_zero() {
        assert_eq!(ZDuration::default().as_std(), Duration::ZERO);
    }

    #[test]
    fn zduration_roundtrip_from_std() {
        let std_d = Duration::from_millis(500);
        let zd = ZDuration::from(std_d);
        let back: Duration = zd.into();
        assert_eq!(back, std_d);
    }

    // --- ZTime ---

    #[test]
    fn ztime_zero_and_default() {
        assert_eq!(ZTime::zero(), ZTime::default());
        assert_eq!(ZTime::zero().as_unix_nanos(), 0);
    }

    #[test]
    fn ztime_from_unix_nanos_negative_clamps_to_zero() {
        assert_eq!(ZTime::from_unix_nanos(-1).as_unix_nanos(), 0);
    }

    #[test]
    fn ztime_from_system_time_roundtrip() {
        let t = ZTime::from_unix_nanos(1_000_000_000);
        let sys = t.to_system_time();
        let back = ZTime::from_system_time(sys);
        assert_eq!(back, t);
    }

    #[test]
    fn ztime_saturating_add_sub() {
        let t = ZTime::from_unix_nanos(5_000_000_000);
        let d = ZDuration::from_secs(2);
        assert_eq!(t.saturating_add(d).as_unix_nanos(), 7_000_000_000);
        assert_eq!(t.saturating_sub(d).as_unix_nanos(), 3_000_000_000);
        // sub below zero saturates
        assert_eq!(ZTime::zero().saturating_sub(d).as_unix_nanos(), 0);
    }

    #[test]
    fn ztime_duration_since() {
        let a = ZTime::from_unix_nanos(5_000_000_000);
        let b = ZTime::from_unix_nanos(3_000_000_000);
        assert_eq!(a.duration_since(b).as_std(), Duration::from_secs(2));
        // saturates to zero when earlier > self
        assert_eq!(b.duration_since(a).as_std(), Duration::ZERO);
    }

    #[test]
    fn ztime_debug_format() {
        let t = ZTime::from_unix_nanos(1_500_000_000);
        let s = format!("{:?}", t);
        assert!(s.contains("secs"));
        assert!(s.contains("nanos"));
    }

    // --- ZClock constructors & kind ---

    #[test]
    fn zclock_system_constructor_and_kind() {
        let c = ZClock::system();
        assert_eq!(c.kind(), ClockKind::System);
    }

    #[test]
    fn zclock_from_kind() {
        assert_eq!(
            ZClock::from_kind(ClockKind::System).kind(),
            ClockKind::System
        );
        assert_eq!(
            ZClock::from_kind(ClockKind::Simulated).kind(),
            ClockKind::Simulated
        );
    }

    #[test]
    fn zclock_debug_format() {
        let s = format!("{:?}", ZClock::system());
        assert!(s.contains("ZClock"));
    }

    #[test]
    fn system_clock_now_is_nonzero() {
        let t = ZClock::system().now();
        assert!(t.as_unix_nanos() > 0);
    }

    // --- set_time ---

    #[test]
    fn set_time_advances_simulated_clock() {
        let clock = ZClock::simulated(ZTime::zero());
        let t = ZTime::from_unix_nanos(1_000_000_000);
        clock.set_time(t).unwrap();
        assert_eq!(clock.now(), t);
    }

    #[test]
    fn set_time_rejects_backwards() {
        let clock = ZClock::simulated(ZTime::from_unix_nanos(1_000_000_000));
        let err = clock.set_time(ZTime::zero()).unwrap_err();
        assert!(matches!(err, ClockError::TimeWentBackwards));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn set_time_on_system_clock_errors() {
        let err = ZClock::system().set_time(ZTime::zero()).unwrap_err();
        assert!(matches!(err, ClockError::NotSimulated));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn advance_on_system_clock_errors() {
        let err = ZClock::system()
            .advance(ZDuration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, ClockError::NotSimulated));
    }

    // --- system clock sleep (just verify it doesn't block) ---

    #[tokio::test]
    async fn system_clock_sleep_zero_completes() {
        ZClock::system().sleep(ZDuration::default()).await;
    }
}
