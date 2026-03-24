#include "gnustep_runtime.h"
#include <stdio.h>

static int a_called = 0, b_called = 0;
static id imp_a(id self, SEL cmd) { (void)cmd; a_called++; return self; }
static id imp_b(id self, SEL cmd) { (void)cmd; b_called++; return self; }

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "SwizzleCls", 0);
    SEL sel_a = sel_registerName("sA");
    SEL sel_b = sel_registerName("sB");
    class_addMethod(cls, sel_a, (IMP)imp_a, "@@:");
    class_addMethod(cls, sel_b, (IMP)imp_b, "@@:");
    objc_registerClassPair(cls);
    struct { void *isa; } raw = { cls };
    id obj = (id)&raw;
    objc_msg_lookup(obj, sel_a);  /* warm cache */
    Method ma = class_getInstanceMethod(cls, sel_a);
    Method mb = class_getInstanceMethod(cls, sel_b);
    method_exchangeImplementations(ma, mb);
    IMP imp = objc_msg_lookup(obj, sel_a);
    imp(obj, sel_a);
    printf("%d %d\n", b_called, a_called);
    return 0;
}
