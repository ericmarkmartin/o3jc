/*
 * gnustep_runtime.h — minimal GNUstep v2 ABI declarations for o3jc tests.
 *
 * Declares only the subset of <objc/runtime.h> + <objc/objc-arc.h> that the
 * Phase 4–N integration tests actually use.  This avoids depending on a
 * system-installed libobjc2 while remaining ABI-compatible with it.
 */

#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Core opaque types ──────────────────────────────────────────────────── */

typedef struct objc_object {
    struct objc_class *isa;
} *id;

typedef struct objc_class  *Class;
typedef struct objc_selector *SEL;
typedef struct objc_method   *Method;
typedef id (*IMP)(id, SEL, ...);
typedef unsigned char BOOL;

#ifndef YES
#  define YES ((BOOL)1)
#endif
#ifndef NO
#  define NO  ((BOOL)0)
#endif
#ifndef nil
#  define nil ((id)0)
#endif
#ifndef Nil
#  define Nil ((Class)0)
#endif

/* ── Selector functions ─────────────────────────────────────────────────── */

SEL         sel_registerName(const char *name);
const char *sel_getName(SEL sel);

/* ── Class functions ────────────────────────────────────────────────────── */

Class objc_allocateClassPair(Class superclass, const char *name, size_t extra);
void  objc_registerClassPair(Class cls);
Class objc_getClass(const char *name);

/* ── Method functions ───────────────────────────────────────────────────── */

BOOL   class_addMethod(Class cls, SEL sel, IMP imp, const char *types);
IMP    class_replaceMethod(Class cls, SEL sel, IMP imp, const char *types);
Method class_getInstanceMethod(Class cls, SEL sel);
IMP    method_getImplementation(Method m);
void   method_exchangeImplementations(Method m1, Method m2);

/* ── Message lookup (GNUstep-style) ─────────────────────────────────────── */

IMP objc_msg_lookup(id receiver, SEL sel);

/* ── Memory management ──────────────────────────────────────────────────── */

id    objc_retain(id obj);
void  objc_release(id obj);
id    objc_autorelease(id obj);
void *objc_autoreleasePoolPush(void);
void  objc_autoreleasePoolPop(void *token);

/* ── Weak references ────────────────────────────────────────────────────── */

id   objc_initWeak(id *location, id value);
id   objc_storeWeak(id *addr, id obj);
id   objc_loadWeakRetained(id *obj);
id   objc_loadWeak(id *obj);
void objc_destroyWeak(id *addr);

#ifdef __cplusplus
}
#endif
