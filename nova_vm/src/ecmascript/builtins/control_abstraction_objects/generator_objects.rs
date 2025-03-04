// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::ops::{Index, IndexMut};

use crate::engine::context::{GcScope, NoGcScope};
use crate::{
    ecmascript::{
        abstract_operations::operations_on_iterator_objects::create_iter_result_object,
        execution::{
            agent::{ExceptionType, JsError},
            Agent, ExecutionContext, JsResult, ProtoIntrinsics,
        },
        types::{
            InternalMethods, InternalSlots, IntoObject, IntoValue, Object, OrdinaryObject, Value,
        },
    },
    engine::{
        rootable::{HeapRootData, Scoped},
        Executable, ExecutionResult, SuspendedVm, Vm,
    },
    heap::{
        indexes::{BaseIndex, GeneratorIndex},
        CompactionLists, CreateHeapData, Heap, HeapMarkAndSweep, WorkQueues,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Generator<'a>(pub(crate) GeneratorIndex<'a>);

impl Generator<'_> {
    /// Unbind this Generator from its current lifetime. This is necessary to use
    /// the Generator as a parameter in a call that can perform garbage
    /// collection.
    pub fn unbind(self) -> Generator<'static> {
        unsafe { core::mem::transmute::<Self, Generator<'static>>(self) }
    }

    // Bind this Generator to the garbage collection lifetime. This enables Rust's
    // borrow checker to verify that your Generators cannot not be invalidated by
    // garbage collection being performed.
    //
    // This function is best called with the form
    // ```rs
    // let array_buffer = array_buffer.bind(&gc);
    // ```
    // to make sure that the unbound Generator cannot be used after binding.
    pub const fn bind<'gc>(self, _: NoGcScope<'gc, '_>) -> Generator<'gc> {
        unsafe { core::mem::transmute::<Self, Generator<'gc>>(self) }
    }

    pub fn scope<'scope>(
        self,
        agent: &mut Agent,
        gc: NoGcScope<'_, 'scope>,
    ) -> Scoped<'scope, Generator<'static>> {
        Scoped::new(agent, self.unbind(), gc)
    }

    pub(crate) const fn _def() -> Self {
        Self(BaseIndex::from_u32_index(0))
    }

    pub(crate) const fn get_index(self) -> usize {
        self.0.into_index()
    }

    /// [27.5.3.3 GeneratorResume ( generator, value, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresume)
    pub(crate) fn resume<'a>(
        self,
        agent: &mut Agent,
        value: Value,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<Object<'a>> {
        let generator = self.bind(gc.nogc());
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match agent[generator].generator_state.as_ref().unwrap() {
            GeneratorState::Suspended { .. } => {
                // 3. Assert: state is either suspended-start or suspended-yield.
            }
            GeneratorState::Executing => {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "The generator is currently running",
                    gc.nogc(),
                ))
            }
            GeneratorState::Completed => {
                // 2. If state is completed, return CreateIterResultObject(undefined, true).
                return Ok(create_iter_result_object(
                    agent,
                    Value::Undefined,
                    true,
                    gc.into_nogc(),
                ));
            }
        };

        // 7. Set generator.[[GeneratorState]] to executing.
        let Some(GeneratorState::Suspended(SuspendedGeneratorState {
            vm_or_args,
            executable,
            execution_context,
        })) = agent[generator]
            .generator_state
            .replace(GeneratorState::Executing)
        else {
            unreachable!()
        };

        // 4. Let genContext be generator.[[GeneratorContext]].
        // 5. Let methodContext be the running execution context.
        // 6. Suspend methodContext.
        // 8. Push genContext onto the execution context stack; genContext is now the running
        // execution context.
        agent.execution_context_stack.push(execution_context);

        let saved = generator.scope(agent, gc.nogc());

        // 9. Resume the suspended evaluation of genContext using NormalCompletion(value) as the
        // result of the operation that suspended it. Let result be the value returned by the
        // resumed computation.
        let execution_result = match vm_or_args {
            VmOrArguments::Arguments(args) => {
                Vm::execute(agent, executable, Some(&args), gc.reborrow())
            }
            VmOrArguments::Vm(vm) => vm.resume(agent, executable, value, gc.reborrow()),
        };

        let generator = saved.get(agent).bind(gc.nogc());

        // GeneratorStart: 4.f. Remove acGenContext from the execution context stack and restore the
        // execution context that is at the top of the execution context stack as the running
        // execution context.
        // GeneratorYield 6 is the same.
        let execution_context = agent.execution_context_stack.pop().unwrap();

        // 10. Assert: When we return here, genContext has already been removed
        // from the execution context stack and methodContext is the currently
        // running execution context.
        // 11. Return ? result.
        match execution_result {
            ExecutionResult::Return(result_value) => {
                // GeneratorStart step 4:
                // g. Set acGenerator.[[GeneratorState]] to completed.
                // h. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with acGenerator can be discarded at this point.
                agent[generator].generator_state = Some(GeneratorState::Completed);
                // i. If result is a normal completion, then
                //    i. Let resultValue be undefined.
                // j. Else if result is a return completion, then
                //    i. Let resultValue be result.[[Value]].
                // l. Return CreateIterResultObject(resultValue, true).
                Ok(create_iter_result_object(
                    agent,
                    result_value,
                    true,
                    gc.into_nogc(),
                ))
            }
            ExecutionResult::Throw(err) => {
                // GeneratorStart step 4:
                // g. Set acGenerator.[[GeneratorState]] to completed.
                // h. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with acGenerator can be discarded at this point.
                agent[generator].generator_state = Some(GeneratorState::Completed);
                // k. i. Assert: result is a throw completion.
                //    ii. Return ? result.
                Err(err)
            }
            ExecutionResult::Yield { vm, yielded_value } => {
                // Yield:
                // 3. Otherwise, return ? GeneratorYield(CreateIterResultObject(value, false)).
                // GeneratorYield:
                // 3. Let generator be the value of the Generator component of genContext.
                // 5. Set generator.[[GeneratorState]] to suspended-yield.
                agent[generator].generator_state =
                    Some(GeneratorState::Suspended(SuspendedGeneratorState {
                        vm_or_args: VmOrArguments::Vm(vm),
                        executable,
                        execution_context,
                    }));
                // 8. Resume callerContext passing NormalCompletion(iterNextObj). ...
                // NOTE: `callerContext` here is the `GeneratorResume` execution context.
                Ok(create_iter_result_object(
                    agent,
                    yielded_value,
                    false,
                    gc.into_nogc(),
                ))
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }

    /// [27.5.3.4 GeneratorResumeAbrupt ( generator, abruptCompletion, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresumeabrupt)
    /// NOTE: This method only accepts throw completions.
    pub(crate) fn resume_throw<'a>(
        self,
        agent: &mut Agent,
        value: Value,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<Object<'a>> {
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match agent[self].generator_state.as_ref().unwrap() {
            GeneratorState::Suspended(SuspendedGeneratorState {
                vm_or_args: VmOrArguments::Arguments(_),
                ..
            }) => {
                // 2. If state is suspended-start, then
                // a. Set generator.[[GeneratorState]] to completed.
                // b. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with generator can be discarded at this point.
                agent[self].generator_state = Some(GeneratorState::Completed);
                // c. Set state to completed.

                // 3. If state is completed, then
                // b. Return ? abruptCompletion.
                return Err(JsError::new(value));
            }
            GeneratorState::Suspended { .. } => {
                // 4. Assert: state is suspended-yield.
            }
            GeneratorState::Executing => {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "The generator is currently running",
                    gc.nogc(),
                ));
            }
            GeneratorState::Completed => {
                // 3. If state is completed, then
                //    b. Return ? abruptCompletion.
                return Err(JsError::new(value));
            }
        };

        // 8. Set generator.[[GeneratorState]] to executing.
        let Some(GeneratorState::Suspended(SuspendedGeneratorState {
            vm_or_args: VmOrArguments::Vm(vm),
            executable,
            execution_context,
        })) = agent[self]
            .generator_state
            .replace(GeneratorState::Executing)
        else {
            unreachable!()
        };

        // 5. Let genContext be generator.[[GeneratorContext]].
        // 6. Let methodContext be the running execution context.
        // 7. Suspend methodContext.
        // 9. Push genContext onto the execution context stack; genContext is now the running
        // execution context.
        agent.execution_context_stack.push(execution_context);

        // 10. Resume the suspended evaluation of genContext using NormalCompletion(value) as the
        // result of the operation that suspended it. Let result be the value returned by the
        // resumed computation.
        let execution_result = vm.resume_throw(agent, executable, value, gc.reborrow());

        // GeneratorStart: 4.f. Remove acGenContext from the execution context stack and restore the
        // execution context that is at the top of the execution context stack as the running
        // execution context.
        // GeneratorYield 6 is the same.
        let execution_context = agent.execution_context_stack.pop().unwrap();

        // 11. Assert: When we return here, genContext has already been removed
        // from the execution context stack and methodContext is the currently
        // running execution context.
        // 12. Return ? result.
        match execution_result {
            ExecutionResult::Return(result) => {
                agent[self].generator_state = Some(GeneratorState::Completed);
                Ok(create_iter_result_object(
                    agent,
                    result,
                    true,
                    gc.into_nogc(),
                ))
            }
            ExecutionResult::Throw(err) => {
                agent[self].generator_state = Some(GeneratorState::Completed);
                Err(err)
            }
            ExecutionResult::Yield { vm, yielded_value } => {
                agent[self].generator_state =
                    Some(GeneratorState::Suspended(SuspendedGeneratorState {
                        vm_or_args: VmOrArguments::Vm(vm),
                        executable,
                        execution_context,
                    }));
                Ok(create_iter_result_object(
                    agent,
                    yielded_value,
                    false,
                    gc.into_nogc(),
                ))
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }
}

impl IntoValue for Generator<'_> {
    fn into_value(self) -> Value {
        self.into()
    }
}

impl<'a> IntoObject<'a> for Generator<'a> {
    fn into_object(self) -> Object<'a> {
        self.into()
    }
}

impl From<Generator<'_>> for Value {
    fn from(val: Generator) -> Self {
        Value::Generator(val.unbind())
    }
}

impl<'a> From<Generator<'a>> for Object<'a> {
    fn from(value: Generator) -> Self {
        Object::Generator(value.unbind())
    }
}

impl TryFrom<Value> for Generator<'_> {
    type Error = ();

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        if let Value::Generator(value) = value {
            Ok(value)
        } else {
            Err(())
        }
    }
}

impl<'a> InternalSlots<'a> for Generator<'a> {
    const DEFAULT_PROTOTYPE: ProtoIntrinsics = ProtoIntrinsics::Generator;

    fn get_backing_object(self, agent: &Agent) -> Option<OrdinaryObject<'static>> {
        agent[self].object_index
    }

    fn set_backing_object(self, agent: &mut Agent, backing_object: OrdinaryObject<'static>) {
        assert!(agent[self]
            .object_index
            .replace(backing_object.unbind())
            .is_none());
    }
}

impl<'a> InternalMethods<'a> for Generator<'a> {}

impl CreateHeapData<GeneratorHeapData, Generator<'static>> for Heap {
    fn create(&mut self, data: GeneratorHeapData) -> Generator<'static> {
        self.generators.push(Some(data));
        Generator(GeneratorIndex::last(&self.generators))
    }
}

impl Index<Generator<'_>> for Agent {
    type Output = GeneratorHeapData;

    fn index(&self, index: Generator) -> &Self::Output {
        &self.heap.generators[index]
    }
}

impl IndexMut<Generator<'_>> for Agent {
    fn index_mut(&mut self, index: Generator) -> &mut Self::Output {
        &mut self.heap.generators[index]
    }
}

impl Index<Generator<'_>> for Vec<Option<GeneratorHeapData>> {
    type Output = GeneratorHeapData;

    fn index(&self, index: Generator) -> &Self::Output {
        self.get(index.get_index())
            .expect("Generator out of bounds")
            .as_ref()
            .expect("Generator slot empty")
    }
}

impl IndexMut<Generator<'_>> for Vec<Option<GeneratorHeapData>> {
    fn index_mut(&mut self, index: Generator) -> &mut Self::Output {
        self.get_mut(index.get_index())
            .expect("Generator out of bounds")
            .as_mut()
            .expect("Generator slot empty")
    }
}

impl TryFrom<HeapRootData> for Generator<'_> {
    type Error = ();

    #[inline]
    fn try_from(value: HeapRootData) -> Result<Self, Self::Error> {
        if let HeapRootData::Generator(value) = value {
            Ok(value)
        } else {
            Err(())
        }
    }
}

impl HeapMarkAndSweep for Generator<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.generators.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.generators.shift_index(&mut self.0)
    }
}

#[derive(Debug, Default)]
pub struct GeneratorHeapData {
    pub(crate) object_index: Option<OrdinaryObject<'static>>,
    pub(crate) generator_state: Option<GeneratorState>,
}

#[derive(Debug)]
pub(crate) enum VmOrArguments {
    Vm(SuspendedVm),
    Arguments(Box<[Value]>),
}

#[derive(Debug)]
pub(crate) struct SuspendedGeneratorState {
    pub(crate) vm_or_args: VmOrArguments,
    pub(crate) executable: Executable,
    pub(crate) execution_context: ExecutionContext,
}

#[derive(Debug)]
pub(crate) enum GeneratorState {
    // SUSPENDED-START has `vm_or_args` set to Arguments, SUSPENDED-YIELD has it set to Vm.
    Suspended(SuspendedGeneratorState),
    Executing,
    Completed,
}

impl HeapMarkAndSweep for SuspendedGeneratorState {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            vm_or_args,
            executable,
            execution_context,
        } = self;
        match vm_or_args {
            VmOrArguments::Vm(vm) => vm.mark_values(queues),
            VmOrArguments::Arguments(args) => args.as_ref().mark_values(queues),
        }
        executable.mark_values(queues);
        execution_context.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            vm_or_args,
            executable,
            execution_context,
        } = self;
        match vm_or_args {
            VmOrArguments::Vm(vm) => vm.sweep_values(compactions),
            VmOrArguments::Arguments(args) => args.as_ref().sweep_values(compactions),
        }
        executable.sweep_values(compactions);
        execution_context.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for GeneratorHeapData {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object_index,
            generator_state,
        } = self;
        object_index.mark_values(queues);
        if let Some(GeneratorState::Suspended(state)) = generator_state {
            state.mark_values(queues);
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            generator_state,
        } = self;
        object_index.sweep_values(compactions);
        if let Some(GeneratorState::Suspended(state)) = generator_state {
            state.sweep_values(compactions);
        }
    }
}
