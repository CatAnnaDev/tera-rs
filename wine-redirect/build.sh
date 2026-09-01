#!/bin/sh
cd "$(dirname "$0")"
clang -dynamiclib -O2 -arch arm64 -arch x86_64 -o libtera_redirect.dylib redirect.c
echo "libtera_redirect.dylib -> $(lipo -archs libtera_redirect.dylib)"
