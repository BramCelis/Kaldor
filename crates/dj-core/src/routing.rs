//! Output routing: which device channels carry master, headphone cue and booth.

use serde::{Deserialize, Serialize};

/// A pair of zero-based device output channels (left, right).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPair {
    pub left: u16,
    pub right: u16,
}

impl ChannelPair {
    pub const fn new(left: u16, right: u16) -> Self {
        Self { left, right }
    }

    /// Highest channel index used, plus one.
    pub fn channels_needed(self) -> u16 {
        self.left.max(self.right) + 1
    }
}

/// Where each bus goes on the output device.
///
/// With the NS4FX the master is on outputs 1-2 and the headphone cue on 3-4
/// (zero-based channels 0-1 and 2-3). Without a controller the master goes
/// to channels 0-1 and the cue is either off or folded into a split-cue
/// signal (cue mono on the left, master mono on the right).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputRouting {
    pub master: ChannelPair,
    pub cue: Option<ChannelPair>,
    pub booth: Option<ChannelPair>,
    /// When true, the master pair carries cue (mono, left) and master
    /// (mono, right) instead of the stereo master. `cue` is ignored.
    pub split_cue: bool,
}

impl Default for OutputRouting {
    fn default() -> Self {
        Self::laptop()
    }
}

impl OutputRouting {
    /// Stereo master on channels 0-1, no cue output.
    pub const fn laptop() -> Self {
        Self {
            master: ChannelPair::new(0, 1),
            cue: None,
            booth: None,
            split_cue: false,
        }
    }

    /// Laptop split cue: cue mono left, master mono right, on channels 0-1.
    pub const fn laptop_split_cue() -> Self {
        Self {
            master: ChannelPair::new(0, 1),
            cue: None,
            booth: None,
            split_cue: true,
        }
    }

    /// Numark NS4FX: master on outputs 1-2, headphones on outputs 3-4.
    pub const fn ns4fx() -> Self {
        Self {
            master: ChannelPair::new(0, 1),
            cue: Some(ChannelPair::new(2, 3)),
            booth: None,
            split_cue: false,
        }
    }

    /// Number of device channels this routing needs.
    pub fn channels_needed(&self) -> u16 {
        let mut n = self.master.channels_needed();
        if !self.split_cue
            && let Some(cue) = self.cue
        {
            n = n.max(cue.channels_needed());
        }
        if let Some(booth) = self.booth {
            n = n.max(booth.channels_needed());
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_counts() {
        assert_eq!(OutputRouting::laptop().channels_needed(), 2);
        assert_eq!(OutputRouting::laptop_split_cue().channels_needed(), 2);
        assert_eq!(OutputRouting::ns4fx().channels_needed(), 4);
        let mut r = OutputRouting::ns4fx();
        r.booth = Some(ChannelPair::new(4, 5));
        assert_eq!(r.channels_needed(), 6);
    }
}
