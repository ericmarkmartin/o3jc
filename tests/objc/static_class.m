// Phase 5: static class loaded from __objc_classes at startup.
// Tests: class loading, selector fixup, method list conversion,
//        root metaclass ISA chain, objc_msgSend trampoline.
#include <stdio.h>
#include "gnustep_runtime.h"

__attribute__((objc_root_class))
@interface TestRoot
+ (void)hello;
@end

@implementation TestRoot
+ (void)hello {
    printf("1\n");
}
@end

int main(void) {
    [TestRoot hello];
    return 0;
}
