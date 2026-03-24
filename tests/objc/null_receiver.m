#include "gnustep_runtime.h"
#include <stdio.h>

int main(void) {
    IMP imp = objc_msg_lookup(NULL, sel_registerName("anything"));
    printf("%d\n", imp == NULL);
    return 0;
}
