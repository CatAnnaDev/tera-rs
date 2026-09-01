#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>

#define DYLD_INTERPOSE(_replacement, _replacee) \
    __attribute__((used)) static struct { const void *replacement; const void *replacee; } \
    _interpose_##_replacee __attribute__((section("__DATA,__interpose"))) = \
    { (const void *)(unsigned long)&_replacement, (const void *)(unsigned long)&_replacee };

static struct sockaddr_in redirect_from;
static struct sockaddr_in redirect_to;
static int redirect_ready = 0;

static int parse_endpoint(const char *text, struct sockaddr_in *out) {
    char buffer[64];
    strncpy(buffer, text, sizeof(buffer) - 1);
    buffer[sizeof(buffer) - 1] = 0;
    char *colon = strrchr(buffer, ':');
    if (!colon) return 0;
    *colon = 0;
    out->sin_family = AF_INET;
    out->sin_port = htons((unsigned short)atoi(colon + 1));
    return inet_pton(AF_INET, buffer, &out->sin_addr) == 1;
}

__attribute__((constructor))
static void tera_redirect_init(void) {
    const char *from = getenv("TERA_REDIRECT_FROM");
    const char *to = getenv("TERA_REDIRECT_TO");
    if (from && to && parse_endpoint(from, &redirect_from) && parse_endpoint(to, &redirect_to)) {
        redirect_ready = 1;
        fprintf(stderr, "[tera-redirect] actif: %s -> %s\n", from, to);
    }
}

static int tera_connect(int fd, const struct sockaddr *address, socklen_t length) {
    if (redirect_ready && address && address->sa_family == AF_INET &&
        length >= (socklen_t)sizeof(struct sockaddr_in)) {
        const struct sockaddr_in *target = (const struct sockaddr_in *)address;
        if (target->sin_addr.s_addr == redirect_from.sin_addr.s_addr &&
            target->sin_port == redirect_from.sin_port) {
            struct sockaddr_in rewritten = *target;
            rewritten.sin_addr = redirect_to.sin_addr;
            rewritten.sin_port = redirect_to.sin_port;
            fprintf(stderr, "[tera-redirect] connexion redirigee vers le proxy\n");
            return connect(fd, (const struct sockaddr *)&rewritten, sizeof(rewritten));
        }
    }
    return connect(fd, address, length);
}

DYLD_INTERPOSE(tera_connect, connect)
