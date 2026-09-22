//! Frame-based animation advanced in fixed simulation ticks.

use serde::{Deserialize, Serialize};

/// Index of a clip within its sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClipId(pub u16);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    /// Indices into the sheet's frames.
    pub frames: Vec<u32>,
    /// Simulation ticks each frame is shown for.
    pub ticks_per_frame: u32,
    pub looping: bool,
    /// Draw the frames mirrored: a right-facing clip from left-facing art.
    #[serde(default)]
    pub flip_x: bool,
}

impl Clip {
    /// RPG Maker walk: with 3 frames, step-stand-step-stand (0,1,2,1); otherwise every frame in order.
    pub fn walk_cycle(
        name: impl Into<String>,
        first_frame: u32,
        frames: u32,
        ticks_per_frame: u32,
    ) -> Self {
        let order: Vec<u32> = if frames == 3 {
            vec![0, 1, 2, 1]
        } else {
            (0..frames).collect()
        };
        Self {
            name: name.into(),
            frames: order.into_iter().map(|f| first_frame + f).collect(),
            ticks_per_frame,
            looping: true,
            flip_x: false,
        }
    }

    /// The standing frame of a walk cycle: the middle one on 3-frame sheets, else the first.
    pub fn idle(name: impl Into<String>, first_frame: u32, frames: u32) -> Self {
        Self {
            name: name.into(),
            frames: vec![first_frame + if frames == 3 { 1 } else { 0 }],
            ticks_per_frame: 1,
            looping: true,
            flip_x: false,
        }
    }
}

/// Playback state of one sprite. Plain data so the host can own and replicate it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationPlayer {
    clip: ClipId,
    step: u32,
    ticks: u32,
    finished: bool,
}

impl AnimationPlayer {
    pub fn new(clip: ClipId) -> Self {
        Self {
            clip,
            ..Self::default()
        }
    }

    pub fn clip(&self) -> ClipId {
        self.clip
    }

    /// Which of the clip's frames is showing, from 0.
    pub fn step(&self) -> u32 {
        self.step
    }

    /// A non-looping clip has shown its last frame for its full duration.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Switches clip. Playing the current clip again does nothing unless `restart`.
    pub fn play(&mut self, clip: ClipId, restart: bool) {
        if clip != self.clip || restart {
            *self = Self::new(clip);
        }
    }

    /// Switches to `clip` keeping the current frame step and timing: for another direction of the
    /// same action, so turning mid-stride or mid-jump does not start the action over.
    pub fn switch_keeping_progress(&mut self, clip: ClipId) {
        self.clip = clip;
    }

    /// Advances one simulation tick.
    pub fn tick(&mut self, clips: &[Clip]) {
        let Some(clip) = clips.get(self.clip.0 as usize) else {
            return;
        };
        if self.finished || clip.frames.is_empty() {
            return;
        }
        self.ticks += 1;
        if self.ticks < clip.ticks_per_frame.max(1) {
            return;
        }
        self.ticks = 0;
        if self.step + 1 < clip.frames.len() as u32 {
            self.step += 1;
        } else if clip.looping {
            self.step = 0;
        } else {
            self.finished = true;
        }
    }

    /// Whether the current clip draws its frames mirrored.
    pub fn flipped(&self, clips: &[Clip]) -> bool {
        clips.get(self.clip.0 as usize).is_some_and(|c| c.flip_x)
    }

    /// The sheet frame to draw now.
    pub fn frame(&self, clips: &[Clip]) -> Option<u32> {
        clips
            .get(self.clip.0 as usize)?
            .frames
            .get(self.step as usize)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clips() -> Vec<Clip> {
        vec![
            Clip::walk_cycle("walk", 0, 3, 2),
            Clip {
                name: "attack".into(),
                frames: vec![10, 11],
                ticks_per_frame: 1,
                looping: false,
                flip_x: false,
            },
        ]
    }

    #[test]
    fn three_frame_walk_ping_pongs() {
        let clips = clips();
        let mut p = AnimationPlayer::new(ClipId(0));
        let mut seen = Vec::new();
        for _ in 0..8 {
            seen.push(p.frame(&clips).unwrap());
            p.tick(&clips);
        }
        assert_eq!(seen, vec![0, 0, 1, 1, 2, 2, 1, 1]);
        assert_eq!(p.frame(&clips), Some(0), "loops back to the start");
    }

    #[test]
    fn one_shot_finishes_on_last_frame() {
        let clips = clips();
        let mut p = AnimationPlayer::new(ClipId(1));
        p.tick(&clips);
        assert_eq!(p.frame(&clips), Some(11));
        assert!(!p.finished());
        p.tick(&clips);
        assert!(p.finished());
        assert_eq!(p.frame(&clips), Some(11));
    }

    #[test]
    fn replaying_same_clip_keeps_progress() {
        let clips = clips();
        let mut p = AnimationPlayer::new(ClipId(0));
        p.tick(&clips);
        p.tick(&clips);
        p.play(ClipId(0), false);
        assert_eq!(p.frame(&clips), Some(1));
        p.play(ClipId(0), true);
        assert_eq!(p.frame(&clips), Some(0));
    }

    #[test]
    fn switching_keeps_progress() {
        let clips = vec![
            Clip::walk_cycle("walk_left", 0, 4, 2),
            Clip::walk_cycle("walk_right", 4, 4, 2),
        ];
        let mut p = AnimationPlayer::new(ClipId(0));
        for _ in 0..5 {
            p.tick(&clips);
        }
        assert_eq!(p.frame(&clips), Some(2));
        p.switch_keeping_progress(ClipId(1));
        assert_eq!(p.frame(&clips), Some(6), "same step of the other direction");
        p.tick(&clips);
        assert_eq!(p.frame(&clips), Some(7), "and the same timing");
    }

    #[test]
    fn idle_frame() {
        assert_eq!(Clip::idle("i", 6, 3).frames, vec![7]);
        assert_eq!(Clip::idle("i", 8, 4).frames, vec![8]);
    }
}
