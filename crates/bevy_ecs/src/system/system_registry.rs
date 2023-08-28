use bevy_utils::HashMap;
use std::marker::PhantomData;

use crate::system::{BoxedSystem, Command, IntoSystem};
use crate::world::{Mut, World};
// Needed for derive(Component) macro
use crate::{self as bevy_ecs};
use bevy_ecs_macros::Resource;

/// Stores initialized [`System`]s, so they can be reused and run in an ad-hoc fashion.
///
/// Systems are keyed by their [`SystemId`]:
///  - repeated calls with the same function type will reuse cached state, including for change detection
///
/// Any [`Commands`](crate::system::Commands) generated by these systems (but not other systems), will immediately be applied.
///
/// This type is stored as a [`Resource`](crate::system::Resource) on each [`World`], initialized by default.
/// However, it will likely be easier to use the corresponding methods on [`World`],
/// to avoid having to worry about split mutable borrows yourself.
///
/// # Limitations
///
///  - stored systems cannot be chained: they can neither have an [`In`](crate::system::In) nor return any values
///  - stored systems cannot recurse: they cannot run other systems via the [`SystemRegistry`] methods on `World` or `Commands`
///  - exclusive systems cannot be used
///
/// # Examples
///
/// You can run a single system directly on the World,
/// applying its effect and caching its state for the next time
/// you call this method.
///
/// ```rust
/// use bevy_ecs::prelude::*;
///
/// let mut world = World::new();  
///
/// #[derive(Default, PartialEq, Debug)]
/// struct Counter(u8);
///
/// fn count_up(mut counter: ResMut<Counter>){
///     counter.0 += 1;
/// }
///
/// world.init_resource::<Counter>();
/// world.run_system(count_up);
///
/// assert_eq!(Counter(1), *world.resource());
/// ```
///
/// These systems immediately apply commands and cache state,
/// ensuring that change detection and [`Local`](crate::system::Local) variables work correctly.
///
/// ```rust
/// use bevy_ecs::prelude::*;
///
/// let mut world = World::new();
///
/// #[derive(Component)]
/// struct Marker;
///
/// fn spawn_7_entities(mut commands: Commands) {
///     for _ in 0..7 {
///         commands.spawn(Marker);
///     }
/// }
///
/// fn assert_7_spawned(query: Query<(), Added<Marker>>){
///     let n_spawned = query.iter().count();
///     assert_eq!(n_spawned, 7);
/// }
///
/// world.run_system(spawn_7_entities);
/// world.run_system(assert_7_spawned);
/// ```
#[derive(Resource, Default)]
pub struct SystemRegistry {
    last_id: u32,
    systems: HashMap<u32, (bool, BoxedSystem)>,
}

/// A wrapper type for TypeId.
/// It identifies a system that is registered in the [`SystemRegistry`].
#[derive(Debug, Clone, Copy)]
pub struct SystemId(u32);

impl SystemRegistry {
    /// Registers a system in the [`SystemRegistry`], so it can be run later.
    ///
    /// Repeatedly registering a system will have no effect.
    #[inline]
    pub fn register<M, S: IntoSystem<(), (), M> + 'static>(&mut self, system: S) -> SystemId {
        let id = self.last_id + 1;
        self.last_id = id;
        self.systems
            .insert(id, (false, Box::new(IntoSystem::into_system(system))));
        SystemId(id)
    }

    /// Removes a registered system from the [`SystemRegistry`], if the [`SystemId`] is not
    /// registered, this function does nothing.
    #[inline]
    pub fn remove(&mut self, id: SystemId) {
        self.systems.remove(&id.0);
    }

    /// Runs the supplied system on the [`World`] a single time.
    ///
    /// You do not need to register systems before they are run in this way.
    /// Instead, systems will be automatically registered and removed when using this function.
    ///
    /// System state will not be reused between runs, so [`Local`](crate::system::Local) variables are not preserved between runs.
    /// To preserve [`Local`](crate::system::Local) variables between runs, it's possible to register and run the system by id manually.
    pub fn run<M, S: IntoSystem<(), (), M> + 'static>(&mut self, world: &mut World, system: S) {
        let mut boxed_system: BoxedSystem = Box::new(IntoSystem::into_system(system));
        boxed_system.initialize(world);
        boxed_system.run((), world);
        boxed_system.apply_deferred(world);
    }

    /// Run the system by its [`SystemId`]
    ///
    /// Systems must be registered before they can be run.
    #[inline]
    pub fn run_by_id(
        &mut self,
        world: &mut World,
        id: SystemId,
    ) -> Result<(), SystemRegistryError> {
        match self.systems.get_mut(&id.0) {
            Some((initialized, matching_system)) => {
                if !*initialized {
                    matching_system.initialize(world);
                    *initialized = true;
                }
                matching_system.run((), world);
                matching_system.apply_deferred(world);
                Ok(())
            }
            None => Err(SystemRegistryError::SystemIdNotRegistered(id)),
        }
    }
}

impl World {
    /// Registers a system in the [`SystemRegistry`]/
    ///
    /// Calls [`SystemRegistry::register`].
    #[inline]
    pub fn register_system<M, S: IntoSystem<(), (), M> + 'static>(
        &mut self,
        system: S,
    ) -> SystemId {
        if !self.contains_resource::<SystemRegistry>() {
            panic!(
                "SystemRegistry not found: Nested and recursive one-shot systems are not supported"
            );
        }

        self.resource_mut::<SystemRegistry>().register(system)
    }

    /// Runs the supplied system on the [`World`] a single time.
    ///
    /// Calls [`SystemRegistry::run_system`].
    #[inline]
    pub fn run_system<M, S: IntoSystem<(), (), M> + 'static>(&mut self, system: S) {
        if !self.contains_resource::<SystemRegistry>() {
            panic!(
                "SystemRegistry not found: Nested and recursive one-shot systems are not supported"
            );
        }

        self.resource_scope(|world, mut registry: Mut<SystemRegistry>| {
            registry.run(world, system);
        });
    }

    /// Run the systems with the provided [`SystemId`].
    ///
    /// Calls [`SystemRegistry::run_by_id`].
    #[inline]
    pub fn run_system_by_id(&mut self, id: SystemId) -> Result<(), SystemRegistryError> {
        if !self.contains_resource::<SystemRegistry>() {
            panic!(
                "SystemRegistry not found: Nested and recursive one-shot systems are not supported"
            );
        }

        self.resource_scope(|world, mut registry: Mut<SystemRegistry>| {
            registry.run_by_id(world, id)
        })
    }
}

/// The [`Command`] type for [`SystemRegistry::run_system`]
#[derive(Debug, Clone)]
pub struct RunSystemCommand<M: Send + Sync + 'static, S: IntoSystem<(), (), M> + Send + Sync + 'static> {
    _phantom_marker: PhantomData<M>,
    system: S,
}

impl<M: Send + Sync + 'static, S: IntoSystem<(), (), M> + Send + Sync + 'static> RunSystemCommand<M, S> {
    /// Creates a new [`Command`] struct, which can be added to [`Commands`](crate::system::Commands)
    #[inline]
    #[must_use]
    pub fn new(system: S) -> Self {
        Self {
            _phantom_marker: PhantomData::default(),
            system,
        }
    }
}

impl<M: Send + Sync + 'static, S: IntoSystem<(), (), M> + Send + Sync + 'static> Command
    for RunSystemCommand<M, S>
{
    #[inline]
    fn apply(self, world: &mut World) {
        world.run_system(self.system);
    }
}

/// The [`Command`] type for [`SystemRegistry::run_by_id`].
#[derive(Debug, Clone)]
pub struct RunSystemById {
    system_id: SystemId,
}

impl RunSystemById {
    /// Creates a new [`Command`] struct, which can be added to [`Commands`](crate::system::Commands)
    pub fn new(system_id: SystemId) -> Self {
        Self { system_id }
    }
}

impl Command for RunSystemById {
    #[inline]
    fn apply(self, world: &mut World) {
        if !world.contains_resource::<SystemRegistry>() {
            panic!(
                "SystemRegistry not found: Nested and recursive one-shot systems are not supported"
            );
        }

        world.resource_scope(|world, mut registry: Mut<SystemRegistry>| {
            registry
                .run_by_id(world, self.system_id)
                // Ideally this error should be handled more gracefully,
                // but that's blocked on a full error handling solution for commands
                .unwrap();
        });
    }
}

/// An operation on a [`SystemRegistry`] failed
#[derive(Debug)]
pub enum SystemRegistryError {
    /// A system was run by label, but no system with that label was found.
    ///
    /// Did you forget to register it?
    SystemIdNotRegistered(SystemId),
}

mod tests {
    use crate as bevy_ecs;
    use crate::prelude::*;

    #[derive(Resource, Default, PartialEq, Debug)]
    struct Counter(u8);

    #[allow(dead_code)]
    fn count_up(mut counter: ResMut<Counter>) {
        counter.0 += 1;
    }

    #[test]
    fn run_system() {
        let mut world = World::new();
        world.init_resource::<Counter>();
        assert_eq!(*world.resource::<Counter>(), Counter(0));
        world.run_system(count_up);
        assert_eq!(*world.resource::<Counter>(), Counter(1));
    }

    #[test]
    /// We need to ensure that the system registry is accessible
    /// even after being used once.
    fn run_two_systems() {
        let mut world = World::new();
        world.init_resource::<Counter>();
        assert_eq!(*world.resource::<Counter>(), Counter(0));
        world.run_system(count_up);
        assert_eq!(*world.resource::<Counter>(), Counter(1));
        world.run_system(count_up);
        assert_eq!(*world.resource::<Counter>(), Counter(2));
    }

    #[allow(dead_code)]
    fn spawn_entity(mut commands: Commands) {
        commands.spawn_empty();
    }

    #[test]
    fn command_processing() {
        let mut world = World::new();
        assert_eq!(world.entities.len(), 0);
        world.run_system(spawn_entity);
        assert_eq!(world.entities.len(), 1);
    }

    #[test]
    fn non_send_resources() {
        fn non_send_count_down(mut ns: NonSendMut<Counter>) {
            ns.0 -= 1;
        }

        let mut world = World::new();
        world.insert_non_send_resource(Counter(10));
        assert_eq!(*world.non_send_resource::<Counter>(), Counter(10));
        world.run_system(non_send_count_down);
        assert_eq!(*world.non_send_resource::<Counter>(), Counter(9));
    }

    #[test]
    fn change_detection() {
        #[derive(Resource, Default)]
        struct ChangeDetector;

        fn count_up_iff_changed(
            mut counter: ResMut<Counter>,
            change_detector: ResMut<ChangeDetector>,
        ) {
            if change_detector.is_changed() {
                counter.0 += 1;
            }
        }

        let mut world = World::new();
        world.init_resource::<ChangeDetector>();
        world.init_resource::<Counter>();
        assert_eq!(*world.resource::<Counter>(), Counter(0));
        // Resources are changed when they are first added.
        let id = world.register_system(count_up_iff_changed);
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(1));
        // Nothing changed
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(1));
        // Making a change
        world.resource_mut::<ChangeDetector>().set_changed();
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(2));
    }

    #[test]
    fn local_variables() {
        // The `Local` begins at the default value of 0
        fn doubling(last_counter: Local<Counter>, mut counter: ResMut<Counter>) {
            counter.0 += last_counter.0 .0;
            last_counter.0 .0 = counter.0;
        }

        let mut world = World::new();
        world.insert_resource(Counter(1));
        assert_eq!(*world.resource::<Counter>(), Counter(1));
        let id = world.register_system(doubling);
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(1));
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(2));
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(4));
        let _ = world.run_system_by_id(id);
        assert_eq!(*world.resource::<Counter>(), Counter(8));
    }

    #[test]
    fn run_system_through_command() {
        use crate::system::commands::Command;
        use crate::system::RunSystemCommand;

        let mut world = World::new();
        let command = RunSystemCommand::new(spawn_entity);
        assert_eq!(world.entities.len(), 0);
        command.apply(&mut world);
        assert_eq!(world.entities.len(), 1);
    }

    #[test]
    // This is a known limitation;
    // if this test passes the docs must be updated
    // to reflect the ability to chain run_system commands
    #[should_panic]
    fn system_recursion() {
        fn count_to_ten(mut counter: ResMut<Counter>, mut commands: Commands) {
            counter.0 += 1;
            if counter.0 < 10 {
                commands.run_system(count_to_ten);
            }
        }

        let mut world = World::new();
        world.init_resource::<Counter>();
        assert_eq!(*world.resource::<Counter>(), Counter(0));
        world.run_system(count_to_ten);
        assert_eq!(*world.resource::<Counter>(), Counter(10));
    }
}
