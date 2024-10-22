// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        abstract_operations::type_conversion::to_boolean,
        builders::builtin_function_builder::BuiltinFunctionBuilder,
        builtins::{
            ordinary::ordinary_create_from_constructor,
            primitive_objects::{PrimitiveObject, PrimitiveObjectData},
            ArgumentsList, Behaviour, Builtin, BuiltinIntrinsicConstructor,
        },
        execution::{Agent, JsResult, ProtoIntrinsics, RealmIdentifier},
        types::{Function, IntoObject, IntoValue, Object, String, Value, BUILTIN_STRING_MEMORY},
    },
    engine::context::Context,
    heap::IntrinsicConstructorIndexes,
};

pub(crate) struct BooleanConstructor;

impl Builtin for BooleanConstructor {
    const NAME: String = BUILTIN_STRING_MEMORY.Boolean;

    const LENGTH: u8 = 1;

    const BEHAVIOUR: Behaviour = Behaviour::Constructor(Self::behaviour);
}
impl BuiltinIntrinsicConstructor for BooleanConstructor {
    const INDEX: IntrinsicConstructorIndexes = IntrinsicConstructorIndexes::Boolean;
}

impl BooleanConstructor {
    fn behaviour(
        agent: Context<'_, '_, '_>,
        _this_value: Value,
        arguments: ArgumentsList,
        new_target: Option<Object>,
    ) -> JsResult<Value> {
        let value = arguments.get(0);
        let b = to_boolean(agent, value);
        let Some(new_target) = new_target else {
            return Ok(b.into());
        };
        let new_target = Function::try_from(new_target).unwrap();
        let o = PrimitiveObject::try_from(ordinary_create_from_constructor(
            agent,
            new_target,
            ProtoIntrinsics::Boolean,
        )?)
        .unwrap();
        agent[o].data = PrimitiveObjectData::Boolean(b);
        Ok(o.into_value())
    }

    pub(crate) fn create_intrinsic(agent: Context<'_, '_, '_>, realm: RealmIdentifier) {
        let intrinsics = agent.get_realm(realm).intrinsics();
        let boolean_prototype = intrinsics.boolean_prototype();

        BuiltinFunctionBuilder::new_intrinsic_constructor::<BooleanConstructor>(agent, realm)
            .with_property_capacity(1)
            .with_prototype_property(boolean_prototype.into_object())
            .build();
    }
}
