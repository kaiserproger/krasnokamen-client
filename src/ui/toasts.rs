//! Vanilla 26.3 toasts (`net.minecraft.client.gui.components.toasts`).
//!
//! `ToastManager` keeps five 32-pixel slots, a FIFO queue and a contiguous
//! free-slot search; `ToastManager$ToastInstance` slides a toast in from the
//! right edge on a 600 ms quadratic curve and owns the fully-visible clock each
//! toast times itself by. The server-reachable kinds are `AdvancementToast`
//! (an advancement completed by a non-reset update whose display has
//! `show_toast`) and `RecipeToast` (recipe-book entries flagged as
//! notifications). Vanilla `Gui.update` advances the manager once per rendered
//! frame on `Util.getMillis`; the session's presentation frame clock does here.
use super::{
    render::Painter,
    state::{NodeAction, Paint, UiState},
};
use crate::{
    api::{MeshGeometry, RenderLayer},
    inventory::{Recipe, Stack},
    protocol::{Result, content::Content},
};
use glam::{Mat4, Vec3};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};

const SLOT_COUNT: usize = 5;
const SLOT_HEIGHT: i32 = 32;
const WIDTH: i32 = 160;
const SLIDE_MS: i64 = 600;
const DISPLAY_MS: f64 = 5000.;
/// Vanilla `Options.notificationDisplayTime` default. The native client has no
/// Notification Time option, so every toast keeps the default duration.
const DISPLAY_TIME_MULTIPLIER: f64 = 1.;
/// Resource bound on toasts waiting for a slot. Vanilla's deque is unbounded;
/// a client without a presentation surface never drains this one.
const QUEUE_LIMIT: usize = 1024;
/// `AdvancementType` wire ordinal of CHALLENGE (TASK 0, CHALLENGE 1, GOAL 2).
const CHALLENGE: i32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Visibility {
    Show,
    Hide,
}
impl Visibility {
    /// `Toast$Visibility` SHOW plays `ui.toast.in`, HIDE `ui.toast.out`.
    fn sound(self) -> &'static str {
        match self {
            Self::Show => "minecraft:ui.toast.in",
            Self::Hide => "minecraft:ui.toast.out",
        }
    }
}

/// `RecipeToast$Entry`: the first stacks of the recipe display's crafting
/// station and result (`SlotDisplay.resolveForFirstStack`; `None` is EMPTY).
#[derive(Clone, Debug)]
pub(crate) struct RecipeEntry {
    pub(crate) station: Option<Stack>,
    pub(crate) result: Option<Stack>,
}

#[derive(Clone, Debug)]
pub(crate) enum ToastKind {
    Advancement {
        title: Arc<Value>,
        frame: i32,
        icon: Option<Stack>,
    },
    Recipe {
        entries: Vec<RecipeEntry>,
        last_changed: i64,
        changed: bool,
        displayed: usize,
    },
    /// `SystemToast`: title and message split at 200 pixels, 12-pixel lines.
    System {
        title: Arc<Value>,
        message: Option<Arc<Value>>,
        title_lines: i32,
        message_lines: i32,
        width: i32,
        display_ms: f64,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Toast {
    pub(crate) kind: ToastKind,
    wanted: Visibility,
}
impl Toast {
    pub(crate) fn advancement(title: Arc<Value>, frame: i32, icon: Option<Stack>) -> Self {
        Self {
            kind: ToastKind::Advancement { title, frame, icon },
            wanted: Visibility::Hide,
        }
    }
    fn recipe() -> Self {
        Self {
            kind: ToastKind::Recipe {
                entries: Vec::new(),
                last_changed: 0,
                changed: false,
                displayed: 0,
            },
            wanted: Visibility::Hide,
        }
    }
    /// `SystemToast(id, title, message)` with its 200-pixel line splits measured:
    /// width is `max(160, widest line, or 200 without lines) + 30`.
    pub(crate) fn system(
        title: Arc<Value>,
        message: Option<Arc<Value>>,
        title_lines: i32,
        message_lines: i32,
        widest_line: Option<i32>,
        display_ms: f64,
    ) -> Self {
        Self {
            kind: ToastKind::System {
                title,
                message,
                title_lines,
                message_lines,
                width: widest_line.unwrap_or(200).max(160) + 30,
                display_ms,
            },
            wanted: Visibility::Hide,
        }
    }
    /// `Toast.width()`/`height()`: 160 x 32 except for `SystemToast`.
    pub(crate) fn width(&self) -> i32 {
        match self.kind {
            ToastKind::System { width, .. } => width,
            _ => WIDTH,
        }
    }
    pub(crate) fn height(&self) -> i32 {
        match self.kind {
            ToastKind::System {
                title_lines,
                message_lines,
                ..
            } => 20 + (title_lines - 1) * 12 + message_lines.max(1) * 12,
            _ => SLOT_HEIGHT,
        }
    }
    /// `Toast.occcupiedSlotCount`: `Mth.positiveCeilDiv(height, 32)`.
    fn slot_count(&self) -> usize {
        (self.height().max(0) as usize).div_ceil(SLOT_HEIGHT as usize)
    }
    fn sound(&self) -> Option<&'static str> {
        match self.kind {
            ToastKind::Advancement { frame, .. } if frame == CHALLENGE => {
                Some("minecraft:ui.toast.challenge_complete")
            }
            _ => None,
        }
    }
    /// `Toast.update(manager, fullyVisibleFor)`.
    fn update(&mut self, visible_for: i64) {
        let display = DISPLAY_MS * DISPLAY_TIME_MULTIPLIER;
        self.wanted = match &mut self.kind {
            ToastKind::Advancement { .. } => {
                if visible_for as f64 >= display {
                    Visibility::Hide
                } else {
                    Visibility::Show
                }
            }
            ToastKind::Recipe {
                entries,
                last_changed,
                changed,
                displayed,
            } => {
                if *changed {
                    *last_changed = visible_for;
                    *changed = false;
                }
                let count = entries.len() as f64;
                // Java d2i of NaN (no entries) is 0.
                *displayed = (visible_for as f64 / (display / count).max(1.) % count) as usize;
                if entries.is_empty() || (visible_for - *last_changed) as f64 >= display {
                    Visibility::Hide
                } else {
                    Visibility::Show
                }
            }
            // Only SystemToast.reset (addOrUpdate) sets lastChanged; the received
            // UNSECURE_SERVER_WARNING is added once, so lastChanged stays 0.
            ToastKind::System { display_ms, .. } => {
                if (visible_for as f64) < *display_ms * DISPLAY_TIME_MULTIPLIER {
                    Visibility::Show
                } else {
                    Visibility::Hide
                }
            }
        };
    }
}

/// `ToastManager$ToastInstance`.
#[derive(Clone, Debug)]
pub(crate) struct ToastInstance {
    pub(crate) serial: u64,
    pub(crate) toast: Toast,
    pub(crate) first_slot: usize,
    slot_count: usize,
    animation_start: i64,
    became_fully_visible_at: i64,
    visibility: Visibility,
    pub(crate) visible_for: i64,
    pub(crate) visible_portion: f32,
    finished: bool,
}
impl ToastInstance {
    fn new(serial: u64, toast: Toast, first_slot: usize, slot_count: usize) -> Self {
        Self {
            serial,
            toast,
            first_slot,
            slot_count,
            animation_start: -1,
            became_fully_visible_at: -1,
            visibility: Visibility::Hide,
            visible_for: 0,
            visible_portion: 0.,
            finished: false,
        }
    }
    fn update(&mut self, now: i64) {
        if self.animation_start == -1 {
            self.animation_start = now;
            self.visibility = Visibility::Show;
        }
        if self.visibility == Visibility::Show && now - self.animation_start <= SLIDE_MS {
            self.became_fully_visible_at = now;
        }
        self.visible_for = now - self.became_fully_visible_at;
        let t = ((now - self.animation_start) as f32 / SLIDE_MS as f32).clamp(0., 1.);
        let t = t * t;
        self.visible_portion = if self.visibility == Visibility::Hide {
            1. - t
        } else {
            t
        };
        self.toast.update(self.visible_for);
        if self.toast.wanted != self.visibility {
            // Reversing mid-slide resumes from the same portion: f2i, then i2l.
            self.animation_start =
                now - ((1. - self.visible_portion) * SLIDE_MS as f32) as i32 as i64;
            self.visibility = self.toast.wanted;
        }
        self.finished =
            self.visibility == Visibility::Hide && now - self.animation_start > SLIDE_MS;
    }
    /// `Toast.xPos(guiWidth, visiblePortion)`.
    pub(crate) fn x(&self, gui_width: f32) -> f32 {
        gui_width - self.toast.width() as f32 * self.visible_portion
    }
    /// `Toast.yPos(firstSlotIndex)`: the slot index times this toast's own height.
    pub(crate) fn y(&self) -> f32 {
        (self.first_slot as i32 * self.toast.height()) as f32
    }
}

/// `ToastManager` without the music `NowPlayingToast` (no music toast option here).
#[derive(Clone, Debug, Default)]
pub(crate) struct ToastManager {
    pub(crate) visible: Vec<ToastInstance>,
    occupied: [bool; SLOT_COUNT],
    queued: VecDeque<Toast>,
    serial: u64,
    /// UI sound events in vanilla play order, drained by the session's audio owner.
    pub(crate) sounds: Vec<&'static str>,
}
impl ToastManager {
    /// `ToastManager.addToast`.
    pub(crate) fn add(&mut self, toast: Toast) {
        if self.queued.len() < QUEUE_LIMIT {
            self.queued.push_back(toast);
        }
    }
    /// `RecipeToast.addOrUpdate`: the first recipe toast on screen, else in the
    /// queue, else a new queued one, takes the entry.
    pub(crate) fn add_recipe(&mut self, entry: RecipeEntry) {
        let existing = self
            .visible
            .iter_mut()
            .map(|instance| &mut instance.toast)
            .chain(self.queued.iter_mut())
            .find(|toast| matches!(toast.kind, ToastKind::Recipe { .. }));
        let toast = match existing {
            Some(toast) => toast,
            None => {
                if self.queued.len() >= QUEUE_LIMIT {
                    return;
                }
                self.queued.push_back(Toast::recipe());
                self.queued.back_mut().expect("pushed above")
            }
        };
        if let ToastKind::Recipe {
            entries, changed, ..
        } = &mut toast.kind
        {
            entries.push(entry);
            *changed = true;
        }
    }
    pub(crate) fn is_idle(&self) -> bool {
        self.visible.is_empty() && self.queued.is_empty()
    }
    /// `ToastManager.update` at `now` milliseconds. Returns whether an instance
    /// was added or removed (the semantic tree must be rebuilt).
    pub(crate) fn update(&mut self, now: i64) -> bool {
        let before = self.visible.len();
        let mut transition_played = false;
        let occupied = &mut self.occupied;
        let sounds = &mut self.sounds;
        self.visible.retain_mut(|instance| {
            let visibility = instance.visibility;
            instance.update(now);
            if instance.visibility != visibility && !transition_played {
                transition_played = true;
                sounds.push(instance.visibility.sound());
            }
            if instance.finished {
                occupied[instance.first_slot..instance.first_slot + instance.slot_count]
                    .fill(false);
            }
            !instance.finished
        });
        let mut changed = self.visible.len() != before;
        if !self.queued.is_empty() && self.occupied.contains(&false) {
            // Deque.removeIf: every queued toast in order, each against the
            // slots the earlier ones took; a sound event plays once per update.
            let mut played: Vec<&'static str> = Vec::new();
            for toast in std::mem::take(&mut self.queued) {
                let count = toast.slot_count();
                let Some(first) = free_run(&self.occupied, count) else {
                    self.queued.push_back(toast);
                    continue;
                };
                self.occupied[first..first + count].fill(true);
                if let Some(sound) = toast.sound()
                    && !played.contains(&sound)
                {
                    played.push(sound);
                    self.sounds.push(sound);
                }
                self.serial += 1;
                self.visible
                    .push(ToastInstance::new(self.serial, toast, first, count));
                changed = true;
            }
        }
        changed
    }
}

/// `ToastManager.findFreeSlotsIndex`: the first run of `count` free slots.
fn free_run(occupied: &[bool; SLOT_COUNT], count: usize) -> Option<usize> {
    if occupied.iter().filter(|slot| !**slot).count() < count {
        return None;
    }
    let mut run = 0;
    for (index, taken) in occupied.iter().enumerate() {
        if *taken {
            run = 0;
        } else {
            run += 1;
            if run == count {
                return Some(index + 1 - count);
            }
        }
    }
    None
}

/// Iteration order of the 26.3 client's progress map: `ByteBufCodecs.map`
/// builds `new HashMap<>(min(n, 65536))` and puts the keys in packet order,
/// then `ClientAdvancements.update` raises toasts while iterating it. Bins
/// keep insertion order across resizes; tree bins (more than eight colliding
/// keys in a table of at least 64) are not modelled. A repeated key keeps its
/// first position.
pub(crate) fn progress_map_order<'a>(ids: &[&'a str]) -> Vec<&'a str> {
    fn java_string_hash(text: &str) -> i32 {
        text.encode_utf16().fold(0i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        })
    }
    let spread = |id: &str| {
        let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
        let hash = 31i32
            .wrapping_mul(java_string_hash(namespace))
            .wrapping_add(java_string_hash(path)) as u32;
        hash ^ (hash >> 16)
    };
    /// HashMap.resize: double the table; split bins keep their relative order.
    fn resize<'a>(bins: &mut Vec<Vec<(&'a str, u32)>>, threshold: &mut usize) {
        let capacity = bins.len() * 2;
        *threshold = if bins.len() >= 16 {
            *threshold * 2
        } else {
            (capacity as f32 * 0.75) as usize
        };
        let old = std::mem::replace(bins, vec![Vec::new(); capacity]);
        for (key, hash) in old.into_iter().flatten() {
            bins[hash as usize & (capacity - 1)].push((key, hash));
        }
    }
    let capacity = ids.len().clamp(1, 65536).next_power_of_two();
    let mut threshold = (capacity as f32 * 0.75) as usize;
    let mut bins: Vec<Vec<(&str, u32)>> = vec![Vec::new(); capacity];
    let mut size = 0;
    for &id in ids {
        let hash = spread(id);
        let mask = bins.len() - 1;
        let bin = &mut bins[hash as usize & mask];
        if bin.iter().any(|(key, _)| *key == id) {
            continue;
        }
        let earlier = bin.len();
        bin.push((id, hash));
        // HashMap.treeifyBin resizes a small table instead of treeifying.
        if earlier >= 8 && bins.len() < 64 {
            resize(&mut bins, &mut threshold);
        }
        size += 1;
        if size > threshold {
            resize(&mut bins, &mut threshold);
        }
    }
    bins.into_iter().flatten().map(|(key, _)| key).collect()
}

fn advancement_type_key(frame: i32) -> &'static str {
    match frame {
        1 => "advancements.toast.challenge",
        2 => "advancements.toast.goal",
        _ => "advancements.toast.task",
    }
}

/// `GuiGraphicsExtractor.text` skips a fully transparent colour.
fn argb(alpha: u32, rgb: u32) -> u32 {
    alpha << 24 | (rgb & 0xffffff)
}

/// `toast/system` (gui scaling nine_slice 160 x 64, border left 17, top 30,
/// right 4, bottom 4) as `GuiGraphicsExtractor.blitNineSlicedSprite` draws it at
/// `width x height`: borders clamp to half the target, corners blit 1:1, edges and
/// centre tile from their top-left with the last tile cropped (no stretch_inner).
/// A system toast is at least 190 wide and never 64 tall, so the general
/// nine-piece branch is the only one it takes.
fn nine_slice_system(p: &mut Painter<'_>, width: i32, height: i32) -> Result<()> {
    const SPRITE: [i32; 2] = [160, 64];
    let texture = p
        .content
        .texture("gui/sprites/toast/system")
        .ok_or("missing verified GUI texture gui/sprites/toast/system")?;
    let blit = |p: &mut Painter<'_>, [x, y, w, h]: [i32; 4], [u, v]: [i32; 2]| {
        let [sw, sh] = SPRITE.map(|n| n as f32);
        p.texture(
            texture,
            [x as f32, y as f32, w as f32, h as f32],
            [
                u as f32 / sw,
                v as f32 / sh,
                (u + w) as f32 / sw,
                (v + h) as f32 / sh,
            ],
            [1.; 3],
            1.,
            0.,
        );
    };
    let tiled =
        |p: &mut Painter<'_>, [x, y, w, h]: [i32; 4], uv: [i32; 2], [tile_w, tile_h]: [i32; 2]| {
            let mut dy = 0;
            while dy < h {
                let mut dx = 0;
                while dx < w {
                    blit(
                        p,
                        [x + dx, y + dy, tile_w.min(w - dx), tile_h.min(h - dy)],
                        uv,
                    );
                    dx += tile_w;
                }
                dy += tile_h;
            }
        };
    let [sw, sh] = SPRITE;
    let (left, right) = (17.min(width / 2), 4.min(width / 2));
    let (top, bottom) = (30.min(height / 2), 4.min(height / 2));
    let (inner_w, inner_h) = (width - right - left, height - bottom - top);
    let (tile_w, tile_h) = (sw - right - left, sh - bottom - top);
    blit(p, [0, 0, left, top], [0, 0]);
    tiled(p, [left, 0, inner_w, top], [left, 0], [tile_w, top]);
    blit(p, [width - right, 0, right, top], [sw - right, 0]);
    blit(p, [0, height - bottom, left, bottom], [0, sh - bottom]);
    tiled(
        p,
        [left, height - bottom, inner_w, bottom],
        [left, sh - bottom],
        [tile_w, bottom],
    );
    blit(
        p,
        [width - right, height - bottom, right, bottom],
        [sw - right, sh - bottom],
    );
    tiled(p, [0, top, left, inner_h], [0, top], [left, tile_h]);
    tiled(
        p,
        [left, top, inner_w, inner_h],
        [left, top],
        [tile_w, tile_h],
    );
    tiled(
        p,
        [width - right, top, right, inner_h],
        [sw - right, top],
        [right, tile_h],
    );
    Ok(())
}

impl UiState {
    /// `ClientAdvancements.update` after a non-reset update whose packet shows
    /// advancements: each progressed advancement that is now done and whose
    /// display has `show_toast` raises an `AdvancementToast`, in the client's
    /// progress-map order.
    pub(super) fn raise_advancement_toasts(&mut self, progressed: &[&str]) {
        for id in progress_map_order(progressed) {
            let Some(advancement) = self.advancements.get(id) else {
                continue;
            };
            if !advancement.done || advancement.flags & 2 == 0 {
                continue;
            }
            let Some(title) = &advancement.display else {
                continue;
            };
            let toast = Toast::advancement(
                Arc::new(title.clone()),
                advancement.frame,
                advancement.icon.clone(),
            );
            self.toasts.add(toast);
        }
    }
    /// `RecipeToast.addOrUpdate` for one recipe-book entry flagged as a notification.
    pub(crate) fn raise_recipe_toast(&mut self, recipe: &Recipe, content: &Content) -> Result<()> {
        let first = |key: &str| -> Result<Option<Stack>> {
            Ok(super::display::display_variants(
                &recipe.display[key],
                key,
                &recipe.display_stacks,
                content,
            )?
            .into_iter()
            .next())
        };
        let entry = RecipeEntry {
            station: first("crafting_station")?,
            result: first("result")?,
        };
        self.toasts.add_recipe(entry);
        Ok(())
    }
    /// `ClientPacketListener.handleLogin`: a server connection whose chat is not
    /// verified raises `SystemToast(UNSECURE_SERVER_WARNING)` (10 s) once per
    /// connection. A state-only client has no fonts to split and measure the
    /// toast with and nothing to draw it on, so it only records the warning.
    pub(crate) fn raise_unsecure_server_toast(&mut self) -> Result<()> {
        if self.insecure_chat_warned {
            return Ok(());
        }
        self.insecure_chat_warned = true;
        if !self.text.can_render() {
            return Ok(());
        }
        let title = json!({"translate": "multiplayer.unsecureserver.toast.title"});
        let message = json!({"translate": "multiplayer.unsecureserver.toast"});
        // SystemToast.splitToLength: Font.split(component, 200); widths are Font.width.
        let title_lines = self.text.layout(&title, 200.)?.line_widths;
        let message_lines = self.text.layout(&message, 200.)?.line_widths;
        let widest = title_lines
            .iter()
            .chain(&message_lines)
            .map(|width| width.ceil() as i32)
            .max();
        self.toasts.add(Toast::system(
            Arc::new(title),
            Some(Arc::new(message)),
            title_lines.len() as i32,
            message_lines.len() as i32,
            widest,
            10_000.,
        ));
        Ok(())
    }
    /// Advance the toast clock to `now` seconds. Returns whether toasts are on
    /// screen or waiting, so the overlay is repainted this frame.
    pub(super) fn advance_toasts(&mut self, now: f64) -> bool {
        if self.toasts.is_idle() {
            return false;
        }
        if self.toasts.update((now * 1000.).floor() as i64) {
            self.dirty = true;
        } else {
            // Only the slide moved: keep the retained nodes, move their bounds.
            for node in &mut self.nodes {
                let Paint::Toast(serial) = node.paint else {
                    continue;
                };
                if let Some(instance) = self.toasts.visible.iter().find(|i| i.serial == serial) {
                    let [_, y, w, h] = node.bounds;
                    node.bounds = [instance.x(self.width), y, w, h];
                    node.visible = instance.visible_portion > 0.;
                }
            }
        }
        true
    }
    pub(crate) fn take_toast_sounds(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.toasts.sounds)
    }
    fn hud_hidden(&self) -> bool {
        self.settings.get("hide_hud").and_then(Value::as_bool) == Some(true)
    }
    /// One semantic node per toast instance (`Hud.isHidden` hides them all).
    pub(super) fn toast_tree(&mut self) -> Result<()> {
        if self.hud_hidden() {
            return Ok(());
        }
        let screen = [0., 0., self.width, self.height];
        for index in 0..self.toasts.visible.len() {
            let instance = &self.toasts.visible[index];
            let serial = instance.serial;
            let bounds = [
                instance.x(self.width),
                instance.y(),
                instance.toast.width() as f32,
                instance.toast.height() as f32,
            ];
            let visible = instance.visible_portion > 0.;
            let label = match &instance.toast.kind {
                ToastKind::Advancement { title, frame, .. } => format!(
                    "{}\n{}",
                    self.text.language.translate(advancement_type_key(*frame)),
                    self.text.language.plain(title)?
                ),
                ToastKind::Recipe { .. } => format!(
                    "{}\n{}",
                    self.text.language.translate("recipe.toast.title"),
                    self.text.language.translate("recipe.toast.description")
                ),
                ToastKind::System { title, message, .. } => {
                    let title = self.text.language.plain(title)?;
                    match message {
                        Some(message) => format!("{title}\n{}", self.text.language.plain(message)?),
                        None => title,
                    }
                }
            };
            self.add(
                format!("toast/{serial}"),
                "status",
                label,
                bounds,
                true,
                None,
                NodeAction::None,
                Paint::Toast(serial),
            )?;
            let node = self.nodes.last_mut().expect("toast node added above");
            node.visible = visible;
            node.clip = Some(screen);
        }
        Ok(())
    }
    /// `ToastManager.extractRenderState`, after the screen and the carried stack.
    pub(super) fn paint_toasts(
        &mut self,
        content: &Content,
        out: &mut MeshGeometry,
        transform: Mat4,
    ) -> Result<()> {
        if self.hud_hidden() {
            return Ok(());
        }
        for instance in &self.toasts.visible {
            if instance.finished {
                continue;
            }
            let origin = transform
                * Mat4::from_translation(Vec3::new(instance.x(self.width), instance.y(), 0.));
            let text = |out: &mut MeshGeometry,
                        component: &Value,
                        wrap: f32,
                        x: f32,
                        y: f32,
                        color: u32|
             -> Result<()> {
                if color >> 24 == 0 {
                    return Ok(());
                }
                let layout = self.text.layout(component, wrap)?;
                self.text.append_layout(
                    content,
                    &layout,
                    origin * Mat4::from_translation(Vec3::new(x, y, 0.)),
                    color,
                    None,
                    out,
                )
            };
            let mut items: Vec<(&Stack, [f32; 4])> = Vec::with_capacity(2);
            match &instance.toast.kind {
                ToastKind::Advancement { title, frame, icon } => {
                    Painter {
                        content,
                        out: &mut *out,
                        transform: origin,
                        light: [-1.; 2],
                        render_layer: RenderLayer::Translucent,
                    }
                    .image(
                        "gui/sprites/toast/advancement",
                        [0., 0., 160., 32.],
                        [0., 0., 1., 1.],
                        [1.; 3],
                        1.,
                    )?;
                    let kind = json!({"translate": advancement_type_key(*frame)});
                    let color = if *frame == CHALLENGE {
                        0xffff88ff
                    } else {
                        0xffffff00
                    };
                    // Font.split(title, 125).
                    let lines = self.text.layout(&**title, 125.)?.line_widths.len();
                    let t = instance.visible_for;
                    if lines == 1 {
                        text(out, &kind, f32::MAX, 30., 7., color)?;
                        text(out, &**title, 125., 30., 18., 0xffffffff)?;
                    } else if t < 1500 {
                        let alpha =
                            (((1500 - t) as f32 / 300.).clamp(0., 1.) * 255.).floor() as u32;
                        text(out, &kind, f32::MAX, 30., 11., argb(alpha, color))?;
                    } else {
                        let alpha =
                            (((t - 1500) as f32 / 300.).clamp(0., 1.) * 252.).floor() as u32;
                        let top = SLOT_HEIGHT / 2 - lines as i32 * 9 / 2;
                        text(out, &**title, 125., 30., top as f32, argb(alpha, 0xffffff))?;
                    }
                    if let Some(icon) = icon {
                        items.push((icon, [8., 8., 16., 16.]));
                    }
                }
                ToastKind::Recipe {
                    entries, displayed, ..
                } => {
                    Painter {
                        content,
                        out: &mut *out,
                        transform: origin,
                        light: [-1.; 2],
                        render_layer: RenderLayer::Translucent,
                    }
                    .image(
                        "gui/sprites/toast/recipe",
                        [0., 0., 160., 32.],
                        [0., 0., 1., 1.],
                        [1.; 3],
                        1.,
                    )?;
                    text(
                        out,
                        &json!({"translate": "recipe.toast.title"}),
                        f32::MAX,
                        30.,
                        7.,
                        0xff500050,
                    )?;
                    text(
                        out,
                        &json!({"translate": "recipe.toast.description"}),
                        f32::MAX,
                        30.,
                        18.,
                        0xff000000,
                    )?;
                    let entry = entries.get(*displayed).ok_or("recipe_toast_entry")?;
                    // The station draws at (3, 3) under a 0.6 scale.
                    if let Some(station) = &entry.station {
                        items.push((station, [1.8, 1.8, 9.6, 9.6]));
                    }
                    if let Some(result) = &entry.result {
                        items.push((result, [8., 8., 16., 16.]));
                    }
                }
                ToastKind::System {
                    title,
                    message,
                    title_lines,
                    ..
                } => {
                    nine_slice_system(
                        &mut Painter {
                            content,
                            out: &mut *out,
                            transform: origin,
                            light: [-1.; 2],
                            render_layer: RenderLayer::Translucent,
                        },
                        instance.toast.width(),
                        instance.toast.height(),
                    )?;
                    // SystemToast.extractTextLines: x 18, 12-pixel lines.
                    let lines = |out: &mut MeshGeometry,
                                 component: &Value,
                                 y: i32,
                                 color: u32|
                     -> Result<()> {
                        let layout = self.text.layout(component, 200.)?;
                        self.text.append_text_layout(
                            &layout,
                            color,
                            &super::render::WorldTextOptions {
                                line_height: 12.,
                                ..Default::default()
                            },
                            &mut Painter {
                                content,
                                out,
                                transform: origin
                                    * Mat4::from_translation(Vec3::new(18., y as f32, 0.)),
                                light: [-1.; 2],
                                render_layer: RenderLayer::Translucent,
                            },
                        )
                    };
                    match message {
                        None => lines(out, title, 12, 0xffffff00)?,
                        Some(message) => {
                            lines(out, title, 7, 0xffffff00)?;
                            lines(out, message, 7 + title_lines * 12, 0xffffffff)?;
                        }
                    }
                }
            }
            for (stack, bounds) in items {
                let model = self.models.icon_model(content, stack, &self.item_context);
                super::state::append_fake_item(content, stack, model, bounds, origin, out)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "toasts_tests.rs"]
mod tests;
