// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::hash::Hasher;

use ahash::AHasher;

use crate::ecmascript::abstract_operations::operations_on_objects::{get, get_method, try_get};
use crate::ecmascript::abstract_operations::testing_and_comparison::is_callable;
use crate::ecmascript::builtins::array::ArrayHeap;
use crate::ecmascript::builtins::keyed_collections::map_objects::map_constructor::add_entries_from_iterable;
use crate::ecmascript::builtins::keyed_collections::map_objects::map_prototype::canonicalize_keyed_collection_key;
use crate::ecmascript::builtins::ordinary::ordinary_create_from_constructor;
use crate::ecmascript::builtins::weak_map::data::WeakMapData;
use crate::ecmascript::builtins::weak_map::WeakMap;
use crate::ecmascript::execution::agent::ExceptionType;
use crate::ecmascript::execution::ProtoIntrinsics;
use crate::ecmascript::types::{Function, IntoFunction, IntoValue};
use crate::engine::context::GcScope;
use crate::engine::TryResult;
use crate::heap::{Heap, PrimitiveHeap, WellKnownSymbolIndexes};
use crate::{
    ecmascript::{
        builders::builtin_function_builder::BuiltinFunctionBuilder,
        builtins::{ArgumentsList, Behaviour, Builtin, BuiltinIntrinsicConstructor},
        execution::{Agent, JsResult, RealmIdentifier},
        types::{IntoObject, Object, String, Value, BUILTIN_STRING_MEMORY},
    },
    heap::IntrinsicConstructorIndexes,
};

use super::weak_map_prototype::WeakMapPrototypeSet;

pub(crate) struct WeakMapConstructor;
impl Builtin for WeakMapConstructor {
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.WeakMap;

    const LENGTH: u8 = 0;

    const BEHAVIOUR: Behaviour = Behaviour::Constructor(Self::constructor);
}

impl BuiltinIntrinsicConstructor for WeakMapConstructor {
    const INDEX: IntrinsicConstructorIndexes = IntrinsicConstructorIndexes::WeakMap;
}

impl WeakMapConstructor {
    fn constructor(
        agent: &mut Agent,
        _: Value,
        arguments: ArgumentsList,
        new_target: Option<Object>,
        mut gc: GcScope,
    ) -> JsResult<Value> {
        // If NewTarget is undefined, throw a TypeError exception.
        let Some(new_target) = new_target else {
            return Err(agent.throw_exception_with_static_message(
                ExceptionType::TypeError,
                "Constructor WeakMap requires 'new'",
                gc.nogc(),
            ));
        };
        let new_target = Function::try_from(new_target).unwrap();
        // 2. Let map be ? OrdinaryCreateFromConstructor(NewTarget, "%WeakMap.prototype%", « [[WeakMapData]] »).
        let mut map = WeakMap::try_from(ordinary_create_from_constructor(
            agent,
            new_target,
            ProtoIntrinsics::WeakMap,
            gc.reborrow(),
        )?)
        .unwrap()
        .unbind()
        .bind(gc.nogc());
        // 3. Set map.[[WeakMapData]] to a new empty List.
        let iterable = arguments.get(0);
        // 4. If iterable is either undefined or null, return map.
        if iterable.is_undefined() || iterable.is_null() {
            Ok(map.into_value())
        } else {
            // Note
            // If the parameter iterable is present, it is expected to be an
            // object that implements an @@iterator method that returns an
            // iterator object that produces a two element array-like object
            // whose first element is a value that will be used as a WeakMap key
            // and whose second element is the value to associate with that
            // key.

            // 5. Let adder be ? Get(map, "set").
            let adder = if let TryResult::Continue(adder) = try_get(
                agent,
                map.into_object().unbind(),
                BUILTIN_STRING_MEMORY.set.to_property_key(),
                gc.nogc(),
            ) {
                adder
            } else {
                let scoped_map = map.scope(agent, gc.nogc());
                let adder = get(
                    agent,
                    map.into_object().unbind(),
                    BUILTIN_STRING_MEMORY.set.to_property_key(),
                    gc.reborrow(),
                )?;
                map = scoped_map.get(agent).bind(gc.nogc());
                adder
            };
            // 6. If IsCallable(adder) is false, throw a TypeError exception.
            let Some(adder) = is_callable(adder, gc.nogc()) else {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "WeakMap.prototype.set is not callable",
                    gc.nogc(),
                ));
            };
            // 7. Return ? AddEntriesFromIterable(map, iterable, adder).
            add_entries_from_iterable_weak_map_constructor(
                agent,
                map.unbind(),
                iterable,
                adder.unbind(),
                gc.reborrow(),
            )
            .map(|result| result.into_value())
        }
    }

    pub(crate) fn create_intrinsic(agent: &mut Agent, realm: RealmIdentifier) {
        let intrinsics = agent.get_realm(realm).intrinsics();
        let weak_map_prototype = intrinsics.weak_map_prototype();

        BuiltinFunctionBuilder::new_intrinsic_constructor::<WeakMapConstructor>(agent, realm)
            .with_property_capacity(1)
            .with_prototype_property(weak_map_prototype.into_object())
            .build();
    }
}

/// ### [24.1.1.2 AddEntriesFromIterable ( target, iterable, adder )](https://tc39.es/ecma262/#sec-add-entries-from-iterable)
///
/// #### Unspecified specialization
///
/// This is a specialization for the `new WeakMap()` use case.
pub fn add_entries_from_iterable_weak_map_constructor<'a>(
    agent: &mut Agent,
    target: WeakMap,
    iterable: Value,
    adder: Function,
    mut gc: GcScope<'a, '_>,
) -> JsResult<WeakMap<'a>> {
    let mut target = target.bind(gc.nogc());
    let mut adder = adder.bind(gc.nogc());
    if let Function::BuiltinFunction(bf) = adder {
        if agent[bf].behaviour == WeakMapPrototypeSet::BEHAVIOUR {
            // Normal WeakMap.prototype.set
            if let Value::Array(iterable) = iterable {
                let scoped_adder = bf.scope(agent, gc.nogc());
                let scoped_target = target.scope(agent, gc.nogc());
                let using_iterator = get_method(
                    agent,
                    iterable.into_value(),
                    WellKnownSymbolIndexes::Iterator.into(),
                    gc.reborrow(),
                )?
                .map(|f| f.unbind())
                .map(|f| f.bind(gc.nogc()));
                target = scoped_target.get(agent).bind(gc.nogc());
                if using_iterator
                    == Some(
                        agent
                            .current_realm()
                            .intrinsics()
                            .array_prototype_values()
                            .into_function(),
                    )
                {
                    let Heap {
                        elements,
                        arrays,
                        bigints,
                        numbers,
                        strings,
                        weak_maps,
                        ..
                    } = &mut agent.heap;
                    let array_heap = ArrayHeap::new(elements, arrays);
                    let primitive_heap = PrimitiveHeap::new(bigints, numbers, strings);

                    // Iterable uses the normal Array iterator of this realm.
                    if iterable.len(&array_heap) == 0 {
                        // Array iterator does not iterate empty arrays.
                        return Ok(scoped_target.get(agent).bind(gc.into_nogc()));
                    }
                    if iterable.is_trivial(&array_heap)
                        && iterable.as_slice(&array_heap).iter().all(|entry| {
                            if let Some(Value::Array(entry)) = *entry {
                                entry.len(&array_heap) == 2
                                    && entry.is_trivial(&array_heap)
                                    && entry.is_dense(&array_heap)
                            } else {
                                false
                            }
                        })
                    {
                        // Trivial, dense array of trivial, dense arrays of two elements.
                        let length = iterable.len(&array_heap);
                        let WeakMapData {
                            keys,
                            values,
                            weak_map_data,
                            ..
                        } = weak_maps[target].borrow_mut(&primitive_heap);
                        let map_data = weak_map_data.get_mut();

                        let length = length as usize;
                        keys.reserve(length);
                        values.reserve(length);
                        // Note: The WeakMap is empty at this point, we don't need the hasher function.
                        assert!(map_data.is_empty());
                        map_data.reserve(length, |_| 0);
                        let hasher = |value: Value| {
                            let mut hasher = AHasher::default();
                            value.hash(&primitive_heap, &mut hasher);
                            hasher.finish()
                        };
                        for entry in iterable.as_slice(&array_heap).iter() {
                            let Some(Value::Array(entry)) = *entry else {
                                unreachable!()
                            };
                            let slice = entry.as_slice(&array_heap);
                            if let Some(value) = slice[0] {
                                if value.is_object() || value.is_symbol() {
                                    let key = canonicalize_keyed_collection_key(numbers, value);
                                    let key_hash = hasher(key);
                                    let value = slice[1].unwrap();
                                    let next_index = keys.len() as u32;
                                    let entry = map_data.entry(
                                        key_hash,
                                        |hash_equal_index| {
                                            keys[*hash_equal_index as usize].unwrap() == key
                                        },
                                        |index_to_hash| {
                                            hasher(keys[*index_to_hash as usize].unwrap())
                                        },
                                    );
                                    match entry {
                                        hashbrown::hash_table::Entry::Occupied(occupied) => {
                                            // We have duplicates in the array. Latter
                                            // ones overwrite earlier ones.
                                            let index = *occupied.get();
                                            values[index as usize] = Some(value);
                                        }
                                        hashbrown::hash_table::Entry::Vacant(vacant) => {
                                            vacant.insert(next_index);
                                            keys.push(Some(key));
                                            values.push(Some(value));
                                        }
                                    }
                                } else {
                                    return Err(agent.throw_exception_with_static_message(
                                        ExceptionType::TypeError,
                                        "WeakMap key must be an Object or Symbol",
                                        gc.nogc(),
                                    ));
                                }
                            }
                        }
                        return Ok(scoped_target.get(agent).bind(gc.into_nogc()));
                    }
                }
                adder = scoped_adder.get(agent).bind(gc.nogc()).into_function();
            }
        }
    }

    Ok(WeakMap::try_from(add_entries_from_iterable(
        agent,
        target.into_object().unbind(),
        iterable,
        adder.unbind(),
        gc,
    )?)
    .unwrap())
}
