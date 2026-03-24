#include "gnustep_runtime.h"
#include <stdio.h>

static id capture(id self, SEL cmd) { (void)cmd; return self; }

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassE", 0);
    SEL sel = sel_registerName("capture");
    class_addMethod(cls, sel, (IMP)capture, "@@:");
    objc_registerClassPair(cls);
    struct { void *isa; } raw = { cls };
    id obj = (id)&raw;
    IMP imp = objc_msg_lookup(obj, sel);
    id ret = imp(obj, sel);
    printf("%d\n", ret == obj);
    return 0;
}
