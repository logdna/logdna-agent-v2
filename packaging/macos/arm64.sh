#!/bin/bash

# VARIABLES
INPUT_TYPE=dir
LICENSE=MIT
OSXPKG_IDENTIFIER_PREFIX=com.logdna
OUTPUT_TYPE=osxpkg
PACKAGE_NAME=logdna-agent
VERSION=1.0
# PAUSE FUNCTION
function pause(){
	read -s -n 1 -p "Press any key to continue . . ."
}

mkdir -p .build-arm/ .pkg-arm/
cp \
	com.logdna.logdna-agent.plist \
	mac-after-install.sh \
	uninstall-mac-agent.sh \
	.build-arm/

cp ../../target/release/logdna-agent .build-arm/${PACKAGE_NAME}

cd .build-arm/
fpm \
	--input-type ${INPUT_TYPE} \
	--output-type ${OUTPUT_TYPE} \
	--name ${PACKAGE_NAME} \
	--version ${VERSION} \
	--license ${LICENSE} \
	--vendor "LogDNA, Inc." \
	--description "LogDNA Agent for Darwin" \
	--url "https://logdna.com/" \
	--maintainer "LogDNA <support@logdna.com>" \
	--after-install ./mac-after-install.sh \
	--osxpkg-identifier-prefix ${OSXPKG_IDENTIFIER_PREFIX} \
	--force \
		./logdna-agent=/usr/local/bin/logdna-agent \
		./com.logdna.logdna-agent.plist=/Library/LaunchDaemons/com.logdna.logdna-agent.plist

cd ../.pkg-arm
mv ../.build-arm/logdna-agent-${VERSION}.pkg logdna-agent-${VERSION}-unsigned.pkg
productsign --sign "Developer ID Installer: Answerbook, Inc. (TT7664HMU3)" logdna-agent-${VERSION}-unsigned.pkg logdna-agent-${VERSION}-arm64.pkg
SHA256CHECKSUM=$(shasum -a 256 logdna-agent-${VERSION}-arm64.pkg | cut -d' ' -f1)
echo "ARM CHECKSUM ${SHA256CHECKSUM}"
cd ..

pause
