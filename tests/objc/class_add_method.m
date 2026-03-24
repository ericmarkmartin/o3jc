#include "gnustep_runtime.h"
#include <stdio.h>

static id noop(id self, SEL cmd) { (void)cmd; return self; }

int main(void) {
    Class cls = objc_allocateClassPair(NULL, "ClassC", 0);
    SEL sel = sel_registerName("doIt");
    BOOL first  = class_addMethod(cls, sel, (IMP)noop, "@@:");
    BOOL second = class_addMethod(cls, sel, (IMP)noop, "@@:");
    printf("%d %d\n", first ? 1 : 0, second ? 1 : 0);
    return 0;
}
