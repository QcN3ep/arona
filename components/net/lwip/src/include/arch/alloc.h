// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_ALLOC_H
#define ARONA_LWIP_ALLOC_H
#include <stddef.h>
void *arona_lwip_alloc(size_t size);
void *arona_lwip_calloc(size_t count, size_t size);
void arona_lwip_free(void *pointer);
#define mem_clib_malloc arona_lwip_alloc
#define mem_clib_calloc arona_lwip_calloc
#define mem_clib_free arona_lwip_free
#endif
