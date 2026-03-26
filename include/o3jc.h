/* Generated with cbindgen:0.27.0 */

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
typedef struct objc_class objc_class;
typedef struct objc_object *Id;


/**
 * This class object is a metaclass.
 * Bit 0, matching Clang's GNUstep v2 codegen (`info = 1` for metaclass).
 */
#define CLASS_IS_METACLASS (1 << 0)

/**
 * The class has been registered and is live in the class table.
 * Runtime-internal flag at a high bit to avoid conflict with ABI flags.
 */
#define CLASS_REGISTERED (1 << 16)

typedef struct Vec_MethodEntry Vec_MethodEntry;

/**
 * The base layout of every Objective-C object.
 * `isa` lives at offset 0 as required by the GNUstep v2 ABI.
 */
typedef struct objc_object {
  objc_class *isa;
} objc_object;

/**
 * Selector descriptor — corresponds to GNUstep's `struct objc_selector`.
 *
 * In the GNUstep v2 ABI, selectors are pointers to `{ name, types }` pairs.
 * The `name` field points to an interned C string (guaranteed unique per
 * selector name by the intern table). Two selectors are equal iff their
 * `name` pointers are equal.
 *
 * Compiled selectors (emitted by Clang into `__objc_selectors`) start with
 * uninterned name pointers; the loader fixes them up at load time.
 */
typedef struct objc_selector {
  /**
   * Interned selector name (stable, process-lifetime pointer).
   */
  const char *name;
  /**
   * Type encoding string, or null if untyped (e.g. from `sel_registerName`).
   */
  const char *types;
} objc_selector;

/**
 * A selector — a non-null pointer to an interned `ObjcSelector`.
 *
 * Two selectors are equal iff their pointer values are equal (guaranteed by
 * the intern table in `sel.rs`). Nullable selectors at call boundaries are
 * expressed as `Option<SEL>`.
 */
typedef struct objc_selector *SEL;

/**
 * A method implementation.
 *
 * Matches the GNUstep ABI signature `id (*IMP)(id, SEL, ...)`. Callers must
 * transmute to the actual parameter types before invoking.
 */
typedef Id (*IMP)(Id, SEL, ...);

/**
 * A single method descriptor stored in a method list.
 *
 * Field order matches Clang's GNUstep v2 ABI: `{ IMP, SEL, types }`.
 */
typedef struct objc_method {
  IMP imp;
  SEL sel;
  /**
   * Type-encoding string (e.g. `"v24@0:8"`), null-terminated.
   */
  const char *types;
} objc_method;

/**
 * A node in the linked chain of method lists attached to a class.
 *
 * The `next` pointer lets categories prepend lists without copying.
 * Phase 1: one list per class; category chaining added in Phase 5.
 */
typedef struct objc_method_list {
  /**
   * Next list in the chain (`None` = end of chain).
   */
  struct objc_method_list *next;
  struct Vec_MethodEntry entries;
} objc_method_list;
