pub mod device_card;
pub mod scanner;
pub mod settings;
pub mod widget;

use gpui_kit::{Hsla, rgb, rgba};

use crate::model::PowerState;

#[derive(Clone, Copy)]
pub struct Palette {
    pub window_bg: Hsla,
    pub window_border: Hsla,
    pub header_bg: Hsla,
    pub card_bg: Hsla,
    pub card_hover_bg: Hsla,
    pub card_border: Hsla,
    pub pill_bg: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub divider: Hsla,
    pub accent: Hsla,
    pub emerald: Hsla,
    pub amber: Hsla,
    pub rose: Hsla,
    pub cyan: Hsla,
    pub slate: Hsla,
}

impl Palette {
    pub fn from_dark(is_dark: bool) -> Self {
        if is_dark {
            Self {
                window_bg: rgba(0x0b1220ea).into(),
                window_border: rgba(0x94a3b838).into(),
                header_bg: rgba(0x111a2ecc).into(),
                card_bg: rgba(0x162238cc).into(),
                card_hover_bg: rgba(0x1e2d48e6).into(),
                card_border: rgba(0x94a3b824).into(),
                pill_bg: rgba(0x33415599).into(),
                text_primary: rgb(0xf8fafc).into(),
                text_secondary: rgb(0x94a3b8).into(),
                text_muted: rgb(0x64748b).into(),
                divider: rgba(0x94a3b826).into(),
                accent: rgb(0x38bdf8).into(),
                emerald: rgb(0x22c55e).into(),
                amber: rgb(0xf59e0b).into(),
                rose: rgb(0xef4444).into(),
                cyan: rgb(0x06b6d4).into(),
                slate: rgb(0x64748b).into(),
            }
        } else {
            Self {
                window_bg: rgba(0xf8fafcf0).into(),
                window_border: rgba(0x0f172a1f).into(),
                header_bg: rgba(0xf1f5f9e0).into(),
                card_bg: rgba(0xffffffe6).into(),
                card_hover_bg: rgba(0xf1f5f9ff).into(),
                card_border: rgba(0x0f172a14).into(),
                pill_bg: rgba(0xe2e8f0cc).into(),
                text_primary: rgb(0x0f172a).into(),
                text_secondary: rgb(0x475569).into(),
                text_muted: rgb(0x64748b).into(),
                divider: rgba(0x0f172a14).into(),
                accent: rgb(0x0284c7).into(),
                emerald: rgb(0x16a34a).into(),
                amber: rgb(0xd97706).into(),
                rose: rgb(0xdc2626).into(),
                cyan: rgb(0x0891b2).into(),
                slate: rgb(0x64748b).into(),
            }
        }
    }

    pub fn status_color(&self, pct: Option<u8>, state: PowerState) -> Hsla {
        if state.is_inactive() {
            return self.slate;
        }
        if state == PowerState::Charging {
            return self.cyan;
        }
        if state == PowerState::Full {
            return self.emerald;
        }
        match pct {
            Some(p) if p >= 50 => self.emerald,
            Some(p) if p >= 20 => self.amber,
            Some(_) => self.rose,
            None => self.slate,
        }
    }
}
