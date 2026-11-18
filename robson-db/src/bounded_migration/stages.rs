//! Private cap1 application-call gate. Never a backend/wire/Ready witness.
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
/// One armed slot plus at most one released-from-slot hook awaiting ack.
pub(super) type Slot = Arc<Mutex<Option<Gate>>>;

use super::{refusal, MigrateError};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Install,
    Begin,
    Ddl(u8),
    Metadata,
    Commit,
    Elapsed,
    Restore,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    BeforeCall,
    AfterReturn,
}
pub(super) struct Gate {
    selected: (Stage, Edge),
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
impl Gate {
    pub(super) fn new(
        stage: Stage,
        edge: Edge,
    ) -> Result<(Self, oneshot::Receiver<()>, oneshot::Sender<()>), MigrateError> {
        if matches!(stage,Stage::Ddl(n) if !(1..=52).contains(&n)) {
            return Err(refusal("invalid migration stage"));
        }
        let (hit, reached) = oneshot::channel();
        let (release, wait) = oneshot::channel();
        Ok((
            Self {
                selected: (stage, edge),
                reached: hit,
                release: wait,
            },
            reached,
            release,
        ))
    }
}
/// A second arm without consumption refuses; no overwrite or queue.
pub(super) fn arm(
    slot: &Slot,
    stage: Stage,
    edge: Edge,
) -> Result<(oneshot::Receiver<()>, oneshot::Sender<()>), MigrateError> {
    let mut armed = slot.lock().map_err(|_| refusal("migration stage poisoned"))?;
    if armed.is_some() {
        return Err(refusal("migration stage already armed"));
    }
    let (gate, hit, release) = Gate::new(stage, edge)?;
    *armed = Some(gate);
    Ok((hit, release))
}
pub(super) async fn checkpoint(slot: &Slot, stage: Stage, edge: Edge) -> Result<(), MigrateError> {
    let gate = {
        let mut armed = slot.lock().map_err(|_| refusal("migration stage poisoned"))?;
        if armed.as_ref().is_some_and(|gate| gate.selected == (stage, edge)) {
            armed.take()
        } else {
            None
        }
    }; // Unlock BEFORE any await; one in-flight hook plus one armed gate<=2.
    if let Some(gate) = gate {
        gate.reached.send(()).map_err(|_| refusal("migration stage observer dropped"))?;
        gate.release.await.map_err(|_| refusal("migration stage release dropped"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn finite_gate_is_cap1_and_requires_the_real_selected_edge() {
        assert!(Gate::new(Stage::Ddl(0), Edge::BeforeCall).is_err());
        assert!(Gate::new(Stage::Ddl(53), Edge::AfterReturn).is_err());
        let slot = Slot::default();
        let (hit, release) = arm(&slot, Stage::Ddl(16), Edge::AfterReturn).unwrap();
        assert!(arm(&slot, Stage::Ddl(32), Edge::AfterReturn).is_err());
        checkpoint(&slot, Stage::Ddl(15), Edge::AfterReturn).await.unwrap();
        assert!(slot.lock().unwrap().is_some());
        let (result, next) =
            tokio::join!(checkpoint(&slot, Stage::Ddl(16), Edge::AfterReturn), async {
                hit.await.unwrap();
                let next = arm(&slot, Stage::Ddl(32), Edge::AfterReturn).unwrap();
                assert!(arm(&slot, Stage::Restore, Edge::AfterReturn).is_err());
                release.send(()).unwrap();
                next
            });
        result.unwrap();
        let (next_hit, next_release) = next;
        let (result, ()) =
            tokio::join!(checkpoint(&slot, Stage::Ddl(32), Edge::AfterReturn), async {
                next_hit.await.unwrap();
                next_release.send(()).unwrap();
            });
        result.unwrap();
        assert!(slot.lock().unwrap().is_none());
    }
}
