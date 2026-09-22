//! SRAM-free, paged Crystal link. All operations are bounded and allocation-free.
pub const MARKER: &[u8; 5] = b"CSD2\x01";
pub const PAYLOAD: usize = 32;
pub const PAGE_BYTES: usize = 38;
pub const FRAME_BYTES: usize = 640;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub epoch: u32,
    pub ticket: u8,
    pub command: u8,
}
pub struct Link {
    pub frame: [u8; FRAME_BYTES],
    queued: Option<Request>,
    active: Option<Request>,
    epoch: u32,
    ack: u8,
}
impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}
impl Link {
    pub const fn new() -> Self {
        Self {
            frame: [0; FRAME_BYTES],
            queued: None,
            active: None,
            epoch: 0,
            ack: 0,
        }
    }
    pub fn submit(&mut self, ticket: u8, command: u8) -> bool {
        if ticket == 0 || command == 0 {
            return false;
        }
        if self.active.is_some_and(|r| r.ticket == ticket) || (self.ack == ticket && command != 10)
        {
            return true;
        }
        // LOCK supersedes an in-flight request; its result must never be published.
        if self.active.is_some() && command != 10 {
            return false;
        }
        self.epoch = self.epoch.wrapping_add(1);
        let r = Request {
            epoch: self.epoch,
            ticket,
            command,
        };
        if command == 10 {
            self.frame.fill(0);
        }
        self.active = Some(r);
        self.queued = Some(r);
        true
    }
    pub fn take(&mut self) -> Option<Request> {
        self.queued.take()
    }
    pub fn finish(&mut self, r: Request, report: &[u8; 128], qr: &[u8]) -> bool {
        if self.active != Some(r) {
            return false;
        }
        self.frame.fill(0);
        self.frame[..128].copy_from_slice(report);
        let n = qr.len().min(512);
        self.frame[128..128 + n].copy_from_slice(&qr[..n]);
        self.ack = r.ticket;
        self.active = None;
        true
    }
    pub fn page(&self, index: u8) -> [u8; PAGE_BYTES] {
        let mut p = [0; PAGE_BYTES];
        p[1] = self.ack;
        p[2] = u8::from(self.active.is_some());
        p[3] = index;
        let start = index as usize * PAYLOAD;
        if start + PAYLOAD <= FRAME_BYTES {
            p[4..36].copy_from_slice(&self.frame[start..start + PAYLOAD]);
        } else {
            p[2] = 2;
        }
        p[36] = 1; // protocol version; bytes 0/37 are publication sequence
        p
    }
}
pub fn marker_offset(bank: &[u8]) -> Option<usize> {
    bank.windows(MARKER.len() + PAGE_BYTES)
        .position(|w| w.starts_with(MARKER))
        .map(|i| i + MARKER.len())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_invalidates_late_completion() {
        let mut s = Link::new();
        assert!(s.submit(1, 8));
        let old = s.take().unwrap();
        assert!(s.submit(2, 10));
        assert!(!s.finish(old, &[42; 128], &[7; 211]));
        assert_eq!(s.frame, [0; FRAME_BYTES]);
        let lock = s.take().unwrap();
        assert!(s.finish(lock, &[0; 128], &[]));
        assert_eq!(s.page(0)[1..3], [2, 0]);
    }
    #[test]
    fn busy_and_duplicate_do_not_execute_twice() {
        let mut s = Link::new();
        assert!(s.submit(1, 20));
        let r = s.take().unwrap();
        assert!(!s.submit(2, 20));
        assert!(s.submit(1, 20));
        assert!(s.take().is_none());
        assert!(s.finish(r, &[0; 128], &[]));
        assert!(s.submit(1, 20));
        assert!(s.take().is_none());
    }
    #[test]
    fn frame_pages_and_bounds() {
        let mut s = Link::new();
        s.submit(1, 12);
        let r = s.take().unwrap();
        s.finish(r, &[3; 128], &[9; 211]);
        assert_eq!(&s.page(3)[4..36], &[3; 32]);
        assert_eq!(&s.page(4)[4..36], &[9; 32]);
        assert_eq!(s.page(20)[2], 2);
        assert!(marker_offset(b"CSD2\x01").is_none());
    }
}
