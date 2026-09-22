//! A small hierarchical task network (HTN) planner.
//!
//! A compound task has methods, tried in order; the first whose condition holds for the
//! current state replaces the task with its subtasks, until only primitive steps are left.
//! Plans are shallow on purpose: the director plans from the world as it is, runs the first
//! step, and plans again when the step ends or can no longer run. Effects are not simulated
//! ahead, which keeps domains short and makes the director adapt to losses at once.

/// A step of a plan: a compound task still to decompose, or a primitive one to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Task<C, P> {
    Compound(C),
    Primitive(P),
}

/// One way of carrying out a compound task.
pub struct Method<S: ?Sized, C, P> {
    /// For logs: which reasoning produced the plan.
    pub name: &'static str,
    pub applies: fn(&S) -> bool,
    pub subtasks: fn(&S) -> Vec<Task<C, P>>,
}

pub trait Domain {
    type State: ?Sized;
    type Compound: Copy + std::fmt::Debug;
    type Primitive: Clone;

    fn methods(
        &self,
        task: Self::Compound,
    ) -> &[Method<Self::State, Self::Compound, Self::Primitive>];
}

/// The primitive steps for `root`, and the names of the methods chosen on the way (outermost
/// first). `None` if some compound task has no applicable method.
pub fn plan<D: Domain>(
    domain: &D,
    state: &D::State,
    root: D::Compound,
) -> Option<(Vec<D::Primitive>, Vec<&'static str>)> {
    const MAX_DEPTH: usize = 32;
    let mut steps = Vec::new();
    let mut reasons = Vec::new();
    let mut stack = vec![(Task::Compound(root), 0usize)];
    while let Some((task, depth)) = stack.pop() {
        match task {
            Task::Primitive(step) => steps.push(step),
            Task::Compound(c) => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                let method = domain.methods(c).iter().find(|m| (m.applies)(state))?;
                reasons.push(method.name);
                // Push in reverse so the first subtask is decomposed first.
                for sub in (method.subtasks)(state).into_iter().rev() {
                    stack.push((sub, depth + 1));
                }
            }
        }
    }
    Some((steps, reasons))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Getting dressed: shoes need socks first, unless barefoot is fine.
    struct Dress;

    #[derive(Clone, Copy, Debug)]
    enum Goal {
        Ready,
        Feet,
    }

    struct Morning {
        cold: bool,
        has_socks: bool,
    }

    type M = Method<Morning, Goal, &'static str>;

    const READY: &[M] = &[Method {
        name: "dress",
        applies: |_| true,
        subtasks: |_| {
            vec![
                Task::Primitive("shirt"),
                Task::Compound(Goal::Feet),
                Task::Primitive("door"),
            ]
        },
    }];
    const FEET: &[M] = &[
        Method {
            name: "warm",
            applies: |s| s.cold && s.has_socks,
            subtasks: |_| vec![Task::Primitive("socks"), Task::Primitive("boots")],
        },
        Method {
            name: "barefoot",
            applies: |s| !s.cold,
            subtasks: |_| vec![],
        },
    ];

    impl Domain for Dress {
        type State = Morning;
        type Compound = Goal;
        type Primitive = &'static str;

        fn methods(&self, task: Goal) -> &[M] {
            match task {
                Goal::Ready => READY,
                Goal::Feet => FEET,
            }
        }
    }

    #[test]
    fn decomposes_in_order_through_the_first_applicable_method() {
        let (steps, why) = plan(
            &Dress,
            &Morning {
                cold: true,
                has_socks: true,
            },
            Goal::Ready,
        )
        .unwrap();
        assert_eq!(steps, vec!["shirt", "socks", "boots", "door"]);
        assert_eq!(why, vec!["dress", "warm"]);

        let (steps, _) = plan(
            &Dress,
            &Morning {
                cold: false,
                has_socks: true,
            },
            Goal::Ready,
        )
        .unwrap();
        assert_eq!(steps, vec!["shirt", "door"]);
    }

    #[test]
    fn no_applicable_method_means_no_plan() {
        let stuck = Morning {
            cold: true,
            has_socks: false,
        };
        assert!(plan(&Dress, &stuck, Goal::Ready).is_none());
    }
}
