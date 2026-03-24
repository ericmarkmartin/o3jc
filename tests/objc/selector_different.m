#include "gnustep_runtime.h"
#include <stdio.h>

int main(void) {
    SEL s1 = sel_registerName("hello");
    SEL s2 = sel_registerName("world");
    printf("%d\n", s1 != s2);
    return 0;
}
