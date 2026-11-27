use bevy::{
    log::{trace_span, warn},
    tasks::{available_parallelism, ComputeTaskPool, TaskPool},
};
use igor_renderer::planetary::SelectionExecutor;

const MAX_SELECTION_WORKERS: usize = 8;

pub(super) struct SelectionWorkers {
    limit: usize,
}

impl SelectionWorkers {
    pub(super) fn from_env() -> Self {
        let requested = std::env::var("IGOR_SELECTION_WORKERS").ok();
        let requested = match requested.as_deref() {
            None | Some("auto") => MAX_SELECTION_WORKERS,
            Some(value) => match value.parse::<usize>() {
                Ok(count @ 1..=MAX_SELECTION_WORKERS) => count,
                _ => {
                    warn!("Invalid IGOR_SELECTION_WORKERS={value:?}; using auto");
                    MAX_SELECTION_WORKERS
                }
            },
        };
        Self {
            limit: requested.min(available_parallelism()),
        }
    }
}

impl SelectionExecutor for SelectionWorkers {
    fn worker_count(&self) -> usize {
        ComputeTaskPool::try_get().map_or(1, |pool| self.limit.min(pool.thread_num() + 1))
    }

    fn for_each<T: Send>(&self, workloads: &mut [T], run: impl Fn(&mut T) + Sync) {
        run_scoped(ComputeTaskPool::get(), workloads, run);
    }
}

fn run_scoped<T: Send>(pool: &TaskPool, workloads: &mut [T], run: impl Fn(&mut T) + Sync) {
    let Some((caller, workers)) = workloads.split_first_mut() else {
        return;
    };
    let mut synchronization = None;
    let run = &run;
    // The calling thread executes one lane. While joining it won't pull
    // unrelated jobs from the global compute executor.
    pool.scope_with_executor(false, None, |scope| {
        for work in workers {
            scope.spawn(async move { run(work) });
        }
        scope.spawn_on_scope(async move { run(caller) });
        synchronization = Some(trace_span!("PlanetScene::synchronize_workloads").entered());
    });
    drop(synchronization);
}

#[cfg(test)]
mod tests {
    use bevy::{
        ecs::system::NonSendMarker,
        prelude::{App, ResMut, Resource, Update},
    };

    use super::*;

    #[derive(Default, Resource)]
    struct Lanes([(u32, Option<std::thread::ThreadId>); 3]);

    #[test]
    fn scoped_executor_joins_all_lanes_from_the_main_thread_system() {
        let caller = std::thread::current().id();
        let pool = ComputeTaskPool::get_or_init(TaskPool::default);
        let mut app = App::new();
        app.init_resource::<Lanes>().add_systems(
            Update,
            move |_: NonSendMarker, mut lanes: ResMut<Lanes>| {
                assert_eq!(std::thread::current().id(), caller);
                run_scoped(pool, &mut lanes.0, |(count, thread)| {
                    *count += 1;
                    *thread = Some(std::thread::current().id());
                });
            },
        );
        app.update();
        let lanes = &app.world().resource::<Lanes>().0;
        assert!(lanes.iter().all(|&(count, _)| count == 1));
        assert_eq!(lanes[0].1, Some(caller));
        assert!(lanes[1..].iter().all(|&(_, thread)| thread != Some(caller)));
    }
}
