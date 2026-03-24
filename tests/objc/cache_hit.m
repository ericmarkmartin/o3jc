#include "gnustep_runtime.h"
#include <stdio.h>

static id noop2(id self, SEL cmd) { (void)cmd; return self; }

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassF", 0);
    SEL sel = sel_registerName("noop2");
    class_addMethod(cls, sel, (IMP)noop2, "@@:");
    objc_registerClassPair(cls);
    struct { void *isa; } raw = { cls };
    id obj = (id)&raw;
    IMP imp1 = objc_msg_lookup(obj, sel);
    IMP imp2 = objc_msg_lookup(obj, sel);
    printf("%d\n", imp1 == imp2);
    return 0;
}
