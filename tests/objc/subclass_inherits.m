#include "gnustep_runtime.h"
#include <stdio.h>

static id greet(id self, SEL cmd) { (void)cmd; return self; }

int main(void) {
    Class parent = objc_allocateClassPair(NULL, "ParentCls", 0);
    SEL sel = sel_registerName("greet");
    class_addMethod(parent, sel, (IMP)greet, "@@:");
    objc_registerClassPair(parent);
    Class child = objc_allocateClassPair(parent, "ChildCls", 0);
    objc_registerClassPair(child);
    struct { void *isa; } raw = { child };
    id obj = (id)&raw;
    IMP imp = objc_msg_lookup(obj, sel);
    printf("%d\n", imp == (IMP)greet);
    return 0;
}
