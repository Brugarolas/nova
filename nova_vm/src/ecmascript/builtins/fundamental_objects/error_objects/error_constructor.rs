// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        abstract_operations::{
            operations_on_objects::{get, has_property},
            type_conversion::to_string,
        },
        builders::builtin_function_builder::BuiltinFunctionBuilder,
        builtins::{
            error::Error, ordinary::ordinary_create_from_constructor, ArgumentsList, Behaviour,
            Builtin, BuiltinIntrinsicConstructor,
        },
        execution::{agent::ExceptionType, Agent, JsResult, ProtoIntrinsics, RealmIdentifier},
        types::{
            Function, IntoObject, IntoValue, Object, PropertyKey, String, Value,
            BUILTIN_STRING_MEMORY,
        },
    },
    engine::context::Context,
    heap::IntrinsicConstructorIndexes,
};

pub(crate) struct ErrorConstructor;

impl Builtin for ErrorConstructor {
    const NAME: String = BUILTIN_STRING_MEMORY.Error;

    const LENGTH: u8 = 1;

    const BEHAVIOUR: Behaviour = Behaviour::Constructor(Self::behaviour);
}
impl BuiltinIntrinsicConstructor for ErrorConstructor {
    const INDEX: IntrinsicConstructorIndexes = IntrinsicConstructorIndexes::Error;
}

impl ErrorConstructor {
    /// ### [20.5.1.1 Error ( message \[ , options \] )](https://tc39.es/ecma262/#sec-error-message)
    fn behaviour(
        agent: Context<'_, '_, '_>,
        _this_value: Value,
        arguments: ArgumentsList,
        new_target: Option<Object>,
    ) -> JsResult<Value> {
        let message = arguments.get(0);
        let options = arguments.get(1);

        // 3. If message is not undefined, then
        let message = if !message.is_undefined() {
            // a. Let msg be ? ToString(message).
            Some(to_string(agent, message)?)
        } else {
            None
        };
        // 4. Perform ? InstallErrorCause(O, options).
        let cause = get_error_cause(agent, options)?;

        // 1. If NewTarget is undefined, let newTarget be the active function object; else let newTarget be NewTarget.
        let new_target = new_target.map_or_else(
            || agent.running_execution_context().function.unwrap(),
            |new_target| Function::try_from(new_target).unwrap(),
        );
        // 2. Let O be ? OrdinaryCreateFromConstructor(newTarget, "%Error.prototype%", « [[ErrorData]] »).
        let o = ordinary_create_from_constructor(agent, new_target, ProtoIntrinsics::Error)?;
        let o = Error::try_from(o).unwrap();
        // b. Perform CreateNonEnumerableDataPropertyOrThrow(O, "message", msg).
        let heap_data = &mut agent[o];
        heap_data.kind = ExceptionType::Error;
        heap_data.message = message;
        heap_data.cause = cause;
        // 5. Return O.
        Ok(o.into_value())
    }

    pub(crate) fn create_intrinsic(agent: Context<'_, '_, '_>, realm: RealmIdentifier) {
        let intrinsics = agent.get_realm(realm).intrinsics();
        let error_prototype = intrinsics.error_prototype();

        BuiltinFunctionBuilder::new_intrinsic_constructor::<ErrorConstructor>(agent, realm)
            .with_property_capacity(1)
            .with_prototype_property(error_prototype.into_object())
            .build();
    }
}

pub(super) fn get_error_cause(
    agent: Context<'_, '_, '_>,
    options: Value,
) -> JsResult<Option<Value>> {
    let Ok(options) = Object::try_from(options) else {
        return Ok(None);
    };
    let key = PropertyKey::from(BUILTIN_STRING_MEMORY.cause);
    if has_property(agent, options, key)? {
        Ok(Some(get(agent, options, key)?))
    } else {
        Ok(None)
    }
}
