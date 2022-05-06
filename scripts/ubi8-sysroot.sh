#!/usr/bin/env bash

TARGET_ARCH="$1"

UBI_MAJOR_VERSION="$2"
UBI_MINOR_VERSION="$3"

UBI_VERSION="${UBI_MAJOR_VERSION}.${UBI_MINOR_VERSION}"

SYSROOT_PATH="/sysroot/ubi-$UBI_VERSION"

ubi_packages="systemd-libs systemd-devel glibc glibc-devel gcc libstdc++-devel libstdc++-static kernel-headers"

apt-get update 1>&2 && apt-get install --no-install-recommends -y dnf 1>&2
dnf install --releasever="${UBI_MAJOR_VERSION}" --forcearch="${TARGET_ARCH}" \
            --installroot="$SYSROOT_PATH/" --repo="ubi-${UBI_MAJOR_VERSION}-baseos" \
            --repo="ubi-${UBI_MAJOR_VERSION}-appstream" --repo="ubi-${UBI_MAJOR_VERSION}-codeready-builder" -y \
            $ubi_packages 1>&2

# Linker file to hint where the linker can find libgcc_s as the packaged symlink is broken \
printf "/* GNU ld script\n*/\n\nOUTPUT_FORMAT(elf64-%s)\n\n GROUP ( /usr/lib64/libgcc_s.so.1  AS_NEEDED ( /usr/lib64/libgcc_s.so.1 ) )" \
       "$(echo "${TARGET_ARCH}" | tr '_' '-' )" > "$SYSROOT_PATH/usr/lib64/libgcc_s.so"

GCC_VERSION=8
# Set up env vars so that the compilers know to link against the target image libraries rather than the base image's
LD_LIBRARY_PATH="-L $SYSROOT_PATH/usr/lib/gcc/${TARGET_ARCH}-redhat-linux/${GCC_VERSION}/ -L $SYSROOT_PATH/usr/lib64"
COMMON_GNU_RUSTFLAGS="-Clink-arg=--sysroot=$SYSROOT_PATH -Clink-arg=-fuse-ld=lld"
COMMON_GNU_CFLAGS="--sysroot $SYSROOT_PATH -isysroot=$SYSROOT_PATH"

printf "CARGO_TARGET_%s_UNKNOWN_LINUX_GNU_RUSTFLAGS=\"%s -Clink-arg=--target=%s-unknown-linux-gnu\"\n" "$(printf "%s" "${TARGET_ARCH}" | tr '[:lower:]' '[:upper:]')" "${COMMON_GNU_RUSTFLAGS}" "${TARGET_ARCH}"

cflags=$(printf "\${CFLAGS_%s_unknown_linux_gnu} %s %s" "${TARGET_ARCH}" "${COMMON_GNU_CFLAGS}" "${LD_LIBRARY_PATH}")
cxxflags=$(printf "\${CXXFLAGS_%s_unknown_linux_gnu} %s" "${TARGET_ARCH}" "${COMMON_GNU_CFLAGS}")

echo "CFLAGS_${TARGET_ARCH}_unknown_linux_gnu=\"${cflags}\""
echo "CXXFLAGS_${TARGET_ARCH}_unknown_linux_gnu=\"${cxxflags}\""

echo EXTRA_CFLAGS=\""${cflags}"\"
echo EXTRA_CXXFLAGS=\""${cxxflags}"\"
echo BINDGEN_EXTRA_CLANG_ARGS=\""${cxxflags}"\"

echo LDFLAGS=\""-fuse-ld=lld"\"
echo SYSTEMD_LIB_DIR=\""$SYSROOT_PATH/lib64"\"
