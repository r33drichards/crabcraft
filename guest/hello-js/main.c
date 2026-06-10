/*
 * crabcraft hello-js: tiny wasi command host around the QuickJS engine.
 *
 * _start protocol (WIRE.md section 3, kind = "command"): read ONE LINE of
 * request JSON from stdin, write the reply JSON line to stdout.
 *
 * The JS logic lives in hello-embed.js, embedded at build time as
 * hello_embed.h (xxd -i). We read all of stdin, set it as the global string
 * `__input`, JS_Eval the script, and print its completion value (the reply
 * JSON string). No quickjs-libc (std/os) is needed, which keeps the build
 * wasi-clean.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "quickjs.h"
#include "hello_embed.h" /* unsigned char hello_embed_js[]; unsigned int hello_embed_js_len; */

/* Print s as a JSON string body (escaped), for the internal-error path. */
static void print_json_escaped(const char *s)
{
    for (; *s; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\')
            printf("\\%c", c);
        else if (c == '\n')
            printf("\\n");
        else if (c < 0x20)
            printf("\\u%04x", c);
        else
            putchar(c);
    }
}

static void print_internal_error(const char *msg)
{
    printf("{\"ok\":false,\"err\":\"internal: ");
    print_json_escaped(msg);
    printf("\"}\n");
}

int main(void)
{
    /* Read all of stdin. */
    size_t cap = 8192, len = 0;
    char *input = malloc(cap);
    if (!input) {
        print_internal_error("out of memory");
        return 1;
    }
    for (;;) {
        if (len == cap) {
            char *p = realloc(input, cap *= 2);
            if (!p) {
                print_internal_error("out of memory");
                return 1;
            }
            input = p;
        }
        size_t n = fread(input + len, 1, cap - len, stdin);
        len += n;
        if (n == 0)
            break;
    }

    JSRuntime *rt = JS_NewRuntime();
    JSContext *ctx = rt ? JS_NewContext(rt) : NULL;
    if (!ctx) {
        print_internal_error("failed to create QuickJS context");
        return 1;
    }

    JSValue glob = JS_GetGlobalObject(ctx);
    JS_SetPropertyStr(ctx, glob, "__input", JS_NewStringLen(ctx, input, len));
    JS_FreeValue(ctx, glob);
    free(input);

    /* JS_Eval wants a NUL-terminated buffer; xxd -i does not add one. */
    char *src = malloc((size_t)hello_embed_js_len + 1);
    if (!src) {
        print_internal_error("out of memory");
        return 1;
    }
    memcpy(src, hello_embed_js, hello_embed_js_len);
    src[hello_embed_js_len] = '\0';

    int rc = 0;
    JSValue v = JS_Eval(ctx, src, hello_embed_js_len, "hello-embed.js",
                        JS_EVAL_TYPE_GLOBAL);
    free(src);
    if (JS_IsException(v)) {
        JSValue exc = JS_GetException(ctx);
        const char *msg = JS_ToCString(ctx, exc);
        print_internal_error(msg ? msg : "unknown exception");
        JS_FreeCString(ctx, msg);
        JS_FreeValue(ctx, exc);
        rc = 1;
    } else {
        const char *s = JS_ToCString(ctx, v);
        if (s) {
            fputs(s, stdout);
            JS_FreeCString(ctx, s);
        } else {
            print_internal_error("reply is not a string");
            rc = 1;
        }
    }
    JS_FreeValue(ctx, v);
    JS_FreeContext(ctx);
    JS_FreeRuntime(rt);
    return rc;
}
