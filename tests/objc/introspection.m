#include "gnustep_runtime.h"
#include <stdio.h>

static id mi(id self, SEL cmd) { (void)cmd; return self; }

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassH", 0);
    SEL sel = sel_registerName("mi");
    class_addMethod(cls, sel, (IMP)mi, "@@:");
    objc_registerClassPair(cls);
    Method m = class_getInstanceMethod(cls, sel);
    printf("%d\n", m != NULL);
    printf("%d\n", method_getImplementation(m) == (IMP)mi);
    return 0;
}
