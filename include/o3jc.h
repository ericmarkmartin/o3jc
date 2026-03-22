/* Generated with cbindgen:0.27.0 */

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
typedef struct objc_class objc_class;

/**
 * The class has been registered and is live in the class table.
 */
#define CLASS_REGISTERED (1 << 0)

/**
 * This class object is a metaclass.
 */
#define CLASS_IS_METACLASS (1 << 1)

typedef struct Vec_MethodEntry Vec_MethodEntry;

/**
 * The base layout of every Objective-C object.
 * `isa` lives at offset 0 as required by the GNUstep v2 ABI.
 */
typedef struct objc_object {
  objc_class *isa;
} objc_object;

/**
 * Opaque selector handle — corresponds to GNUstep's `struct objc_selector`.
 *
 *
 * Never constructed directly; exists only as a pointee for `SEL`. The
 * concrete representation (an interned C string address) is a runtime
 * implementation detail invisible to Clang-compiled code.
 */
typedef struct objc_selector {
  uint8_t _private[0];
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
 * An opaque object reference (`id` in Objective-C).
 *
 * `None` is the equivalent of Objective-C `nil`. `Option<NonNull<T>>` has the
 * null-pointer niche optimization, so it is ABI-compatible with `*mut T` at
 * the C boundary.
 */
typedef struct objc_object *Id;

/**
 * A method implementation.
 *
 * Matches the GNUstep ABI signature `id (*IMP)(id, SEL, ...)`. Callers must
 * transmute to the actual parameter types before invoking.
 */
typedef Id (*IMP)(Id, SEL, ...);

/**
 * A single method descriptor stored in a method list.
 */
typedef struct objc_method {
  SEL sel;
  /**
   * Type-encoding string (e.g. `"v24@0:8"`), null-terminated.
   */
  const char *types;
  IMP imp;
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
