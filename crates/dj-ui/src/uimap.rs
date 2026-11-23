//! A record of where every control was drawn in the last frame, by name.
//! Tests use it to check that controls exist, have a size, stay inside
//! their region and the window, and do not overlap other regions; it is
//! cheap (a reused `Vec`) and has no effect on what is drawn.

use egui::Rect;

/// Screen regions (see `layout.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Region {
    TopBar,
    LeftDecks,
    RightDecks,
    Waveforms,
    /// The FX panel between the decks.
    Fx,
    MixerBar,
    Mixer,
    Divider,
    Bottom,
    /// The settings window over the app.
    Settings,
}

/// One control as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Stable name, for example "play", "trim", "search".
    pub name: &'static str,
    /// Deck number 1-4 for per-deck controls.
    pub deck: Option<u8>,
    pub region: Region,
    pub rect: Rect,
    /// Clip rectangle it was drawn with.
    pub clip: Rect,
    /// Inside a scroll area (may be partly scrolled out of view).
    pub scrolled: bool,
}

/// Everything recorded in one frame.
#[derive(Clone, Debug)]
pub struct UiMap {
    pub screen: Rect,
    pub regions: Vec<(Region, Rect)>,
    pub controls: Vec<Placed>,
    pub(crate) region: Option<Region>,
    pub(crate) scrolled: bool,
}

impl Default for UiMap {
    fn default() -> Self {
        Self {
            screen: Rect::NOTHING,
            regions: Vec::new(),
            controls: Vec::new(),
            region: None,
            scrolled: false,
        }
    }
}

impl UiMap {
    pub(crate) fn begin(&mut self, screen: Rect) {
        self.screen = screen;
        self.regions.clear();
        self.controls.clear();
        self.region = None;
        self.scrolled = false;
    }

    pub(crate) fn enter(&mut self, region: Region, rect: Rect) {
        self.regions.push((region, rect));
        self.region = Some(region);
        self.scrolled = false;
    }

    pub(crate) fn add(
        &mut self,
        ui: &egui::Ui,
        name: &'static str,
        deck: Option<usize>,
        rect: Rect,
    ) {
        if let Some(region) = self.region {
            self.controls.push(Placed {
                name,
                deck: deck.map(|d| d as u8 + 1),
                region,
                rect,
                clip: ui.clip_rect(),
                scrolled: self.scrolled,
            });
        }
    }

    /// Like [`Self::add`], for a control drawn where `self` could not be
    /// borrowed (inside a window); `clip` is that window's clip rectangle.
    pub(crate) fn add_rect(&mut self, name: &'static str, rect: Rect, clip: Rect) {
        if let Some(region) = self.region {
            self.controls.push(Placed {
                name,
                deck: None,
                region,
                rect,
                clip,
                scrolled: false,
            });
        }
    }

    /// The rectangle of a region in the last frame.
    pub fn region(&self, r: Region) -> Option<Rect> {
        self.regions
            .iter()
            .find(|(x, _)| *x == r)
            .map(|(_, rect)| *rect)
    }

    /// All controls with this name (and deck, if given).
    pub fn find(&self, name: &str, deck: Option<u8>) -> Vec<&Placed> {
        self.controls
            .iter()
            .filter(|c| c.name == name && (deck.is_none() || c.deck == deck))
            .collect()
    }
}
