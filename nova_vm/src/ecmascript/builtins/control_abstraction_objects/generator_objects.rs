// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::ops::{Index, IndexMut};

use crate::{
    ecmascript::{
        abstract_operations::operations_on_iterator_objects::create_iter_result_object,
        execution::{
            agent::{ExceptionType, JsError},
            Agent, ExecutionContext, JsResult, ProtoIntrinsics,
        },
        types::{
            InternalMethods, InternalSlots, IntoObject, IntoValue, Object, ObjectHeapData,
            OrdinaryObject, Value,
        },
    },
    engine::{Executable, ExecutionResult, Vm},
    heap::{
        indexes::{BaseIndex, GeneratorIndex},
        CompactionLists, CreateHeapData, Heap, HeapMarkAndSweep, WorkQueues,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Generator<'gen>(pub(crate) GeneratorIndex<'gen>);

impl<'gen> Generator<'gen> {
    pub(crate) const fn _def() -> Self {
        Self(BaseIndex::from_u32_index(0))
    }

    pub(crate) const fn get_index(self) -> usize {
        self.0.into_index()
    }

    /// [27.5.3.3 GeneratorResume ( generator, value, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresume)
    pub(crate) fn resume(self, agent: &mut Agent<'gen>, value: Value<'gen>) -> JsResult<'gen, Object<'gen>> {
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match agent[self].generator_state.as_ref().unwrap() {
            GeneratorState::Suspended { .. } => {
                // 3. Assert: state is either suspended-start or suspended-yield.
            }
            GeneratorState::Executing => {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "The generator is currently running",
                ))
            }
            GeneratorState::Completed => {
                // 2. If state is completed, return CreateIterResultObject(undefined, true).
                return Ok(create_iter_result_object(agent, Value::Undefined, true));
            }
        };

        // 7. Set generator.[[GeneratorState]] to executing.
        let Some(GeneratorState::Suspended {
            vm,
            executable,
            execution_context,
        }) = agent[self]
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

        // 9. Resume the suspended evaluation of genContext using NormalCompletion(value) as the
        // result of the operation that suspended it. Let result be the value returned by the
        // resumed computation.
        let execution_result = match vm {
            None => Vm::execute(agent, &executable),
            Some(vm) => vm.resume(agent, &executable, value),
        };

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
                agent[self].generator_state = Some(GeneratorState::Completed);
                // i. If result is a normal completion, then
                //    i. Let resultValue be undefined.
                // j. Else if result is a return completion, then
                //    i. Let resultValue be result.[[Value]].
                // l. Return CreateIterResultObject(resultValue, true).
                Ok(create_iter_result_object(agent, result_value, true))
            }
            ExecutionResult::Throw(err) => {
                // GeneratorStart step 4:
                // g. Set acGenerator.[[GeneratorState]] to completed.
                // h. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with acGenerator can be discarded at this point.
                agent[self].generator_state = Some(GeneratorState::Completed);
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
                agent[self].generator_state = Some(GeneratorState::Suspended {
                    vm: Some(vm),
                    executable,
                    execution_context,
                });
                // 8. Resume callerContext passing NormalCompletion(iterNextObj). ...
                // NOTE: `callerContext` here is the `GeneratorResume` execution context.
                Ok(create_iter_result_object(agent, yielded_value, false))
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }

    /// [27.5.3.4 GeneratorResumeAbrupt ( generator, abruptCompletion, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresumeabrupt)
    /// NOTE: This method only accepts throw completions.
    pub(crate) fn resume_throw(self, agent: &mut Agent<'gen>, value: Value<'gen>) -> JsResult<'gen, Object<'gen>> {
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match agent[self].generator_state.as_ref().unwrap() {
            GeneratorState::Suspended { vm: None, .. } => {
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
                ));
            }
            GeneratorState::Completed => {
                // 3. If state is completed, then
                //    b. Return ? abruptCompletion.
                return Err(JsError::new(value));
            }
        };

        // 8. Set generator.[[GeneratorState]] to executing.
        let Some(GeneratorState::Suspended {
            vm: Some(vm),
            executable,
            execution_context,
        }) = agent[self]
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
        let execution_result = vm.resume_throw(agent, &executable, value);

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
                Ok(create_iter_result_object(agent, result, true))
            }
            ExecutionResult::Throw(err) => {
                agent[self].generator_state = Some(GeneratorState::Completed);
                Err(err)
            }
            ExecutionResult::Yield { vm, yielded_value } => {
                agent[self].generator_state = Some(GeneratorState::Suspended {
                    vm: Some(vm),
                    executable,
                    execution_context,
                });
                Ok(create_iter_result_object(agent, yielded_value, false))
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }
}

impl<'gen> From<Generator<'gen>> for GeneratorIndex<'gen> {
    fn from(val: Generator<'gen>) -> Self {
        val.0
    }
}

impl<'gen> From<GeneratorIndex<'gen>> for Generator<'gen> {
    fn from(value: GeneratorIndex<'gen>) -> Self {
        Self(value)
    }
}

impl<'gen> IntoValue<'gen> for Generator<'gen> {
    fn into_value(self) -> Value<'gen> {
        self.into()
    }
}

impl<'gen> IntoObject<'gen> for Generator<'gen> {
    fn into_object(self) -> Object<'gen> {
        self.into()
    }
}

impl<'gen> From<Generator<'gen>> for Value<'gen> {
    fn from(val: Generator<'gen>) -> Self {
        Value::Generator(val)
    }
}

impl<'gen> From<Generator<'gen>> for Object<'gen> {
    fn from(value: Generator<'gen>) -> Self {
        Object::Generator(value)
    }
}

impl<'a> InternalSlots<'a> for Generator<'a> {
    const DEFAULT_PROTOTYPE: ProtoIntrinsics = ProtoIntrinsics::Generator;

    fn get_backing_object<'b>(self, agent: &'b Agent<'a>) -> Option<OrdinaryObject<'a>> where 'a: 'b {
        agent[self].object_index
    }

    fn create_backing_object<'b>(self, agent: &'b mut Agent<'a>) -> OrdinaryObject<'a> where 'a: 'b {
        let prototype = agent
            .current_realm()
            .intrinsics()
            .get_intrinsic_default_proto(Self::DEFAULT_PROTOTYPE);
        let backing_object = agent.heap.create(ObjectHeapData {
            extensible: true,
            prototype: Some(prototype),
            keys: Default::default(),
            values: Default::default(),
        });
        agent[self].object_index = Some(backing_object);
        backing_object
    }
}

impl<'gen> InternalMethods<'gen> for Generator<'gen> {}

impl<'gen> CreateHeapData<GeneratorHeapData<'gen>, Generator<'gen>> for Heap<'gen> {
    fn create(&mut self, data: GeneratorHeapData<'gen>) -> Generator<'gen> {
        self.generators.push(Some(data));
        Generator(GeneratorIndex::last(&self.generators))
    }
}

impl<'gen> Index<Generator<'gen>> for Agent<'gen> {
    type Output = GeneratorHeapData<'gen>;

    fn index(&self, index: Generator<'gen>) -> &Self::Output {
        &self.heap.generators[index]
    }
}

impl<'gen> IndexMut<Generator<'gen>> for Agent<'gen> {
    fn index_mut(&mut self, index: Generator<'gen>) -> &mut Self::Output {
        &mut self.heap.generators[index]
    }
}

impl<'gen> Index<Generator<'gen>> for Vec<Option<GeneratorHeapData<'gen>>> {
    type Output = GeneratorHeapData<'gen>;

    fn index(&self, index: Generator<'gen>) -> &Self::Output {
        self.get(index.get_index())
            .expect("Generator out of bounds")
            .as_ref()
            .expect("Generator slot empty")
    }
}

impl<'gen> IndexMut<Generator<'gen>> for Vec<Option<GeneratorHeapData<'gen>>> {
    fn index_mut(&mut self, index: Generator<'gen>) -> &mut Self::Output {
        self.get_mut(index.get_index())
            .expect("Generator out of bounds")
            .as_mut()
            .expect("Generator slot empty")
    }
}

impl<'gen> HeapMarkAndSweep<'gen> for Generator<'gen> {
    fn mark_values(&self, queues: &mut WorkQueues<'gen>) {
        queues.generators.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.generators.shift_index(&mut self.0)
    }
}

#[derive(Debug, Default)]
pub struct GeneratorHeapData<'gen> {
    pub(crate) object_index: Option<OrdinaryObject<'gen>>,
    pub(crate) generator_state: Option<GeneratorState<'gen>>,
}

#[derive(Debug)]
pub(crate) enum GeneratorState<'gen> {
    // SUSPENDED-START has `vm` set to None, SUSPENDED-YIELD has it set to Some.
    Suspended {
        vm: Option<Vm<'gen>>,
        executable: Executable<'gen>,
        execution_context: ExecutionContext<'gen>,
    },
    Executing,
    Completed,
}

impl<'gen> HeapMarkAndSweep<'gen> for GeneratorHeapData<'gen> {
    fn mark_values(&self, queues: &mut WorkQueues<'gen>) {
        self.object_index.mark_values(queues);
        if let Some(GeneratorState::Suspended {
            vm,
            executable,
            execution_context,
        }) = &self.generator_state
        {
            vm.mark_values(queues);
            executable.mark_values(queues);
            execution_context.mark_values(queues);
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        self.object_index.sweep_values(compactions);
        if let Some(GeneratorState::Suspended {
            vm,
            executable,
            execution_context,
        }) = &mut self.generator_state
        {
            vm.sweep_values(compactions);
            executable.sweep_values(compactions);
            execution_context.sweep_values(compactions);
        }
    }
}
