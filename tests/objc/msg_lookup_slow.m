#include "gnustep_runtime.h"
#include <stdio.h>

static id echo(id self, SEL cmd) { (void)cmd; return self; }

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassD", 0);
    SEL sel = sel_registerName("echo");
    class_addMethod(cls, sel, (IMP)echo, "@@:");
    objc_registerClassPair(cls);
    struct { void *isa; } raw = { cls };
    id obj = (id)&raw;
    IMP imp = objc_msg_lookup(obj, sel);
    printf("%d\n", imp != NULL);
    return 0;
}
