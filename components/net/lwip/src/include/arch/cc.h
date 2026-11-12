// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_CC_H
#define ARONA_LWIP_CC_H

#include <stddef.h>
#include <stdint.h>
#include <limits.h>
#include "arch/alloc.h"

#define LWIP_NO_INTTYPES_H 1
#define LWIP_NO_CTYPE_H 1
#define LWIP_NO_UNISTD_H 1
#define BYTE_ORDER LITTLE_ENDIAN
#define SSIZE_MAX __PTRDIFF_MAX__
typedef ptrdiff_t ssize_t;
#define X8_F "02x"
#define U16_F "u"
#define S16_F "d"
#define X16_F "x"
#define U32_F "u"
#define S32_F "d"
#define X32_F "x"
#define SZT_F "zu"

_Static_assert(sizeof(void *) == 8 && sizeof(size_t) == 8, "native pointer ABI");
_Static_assert(__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__, "native byte order");

int *arona_lwip_errno(void);
_Noreturn void arona_lwip_assert(const char *message, const char *file, int line);
uint32_t arona_lwip_random(void);
void arona_lwip_yield(void);
void arona_lwip_mark_core(void);
void arona_lwip_check_core(void);

#define LWIP_PROVIDE_ERRNO
#define errno (*arona_lwip_errno())
#define LWIP_PLATFORM_DIAG(args) do { } while (0)
#define LWIP_PLATFORM_ASSERT(message) arona_lwip_assert(message, __FILE__, __LINE__)
#define LWIP_RAND() arona_lwip_random()

#endif
