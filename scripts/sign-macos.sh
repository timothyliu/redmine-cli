#!/bin/sh
# Signs ~/.cargo/bin/redmine with a stable self-signed identity so the keychain's
# "Always Allow" survives reinstalls. Creates the identity once, in the login keychain.
set -eu

NAME=redmine-cli-codesign
BIN="${1:-$HOME/.cargo/bin/redmine}"

if ! security find-identity -p codesigning | grep -q "\"$NAME\""; then
    dir=$(mktemp -d)
    trap 'rm -rf "$dir"' EXIT
    cat > "$dir/cs.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
    # macOS's LibreSSL: a p12 from OpenSSL 3+ fails to import ("MAC verification failed").
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -config "$dir/cs.cnf" -keyout "$dir/key.pem" -out "$dir/cert.pem" 2>/dev/null
    /usr/bin/openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" \
        -name "$NAME" -passout pass:import -out "$dir/cs.p12"
    security import "$dir/cs.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
        -P import -T /usr/bin/codesign
    echo "created identity $NAME in the login keychain"
fi

codesign --force -s "$NAME" --identifier redmine-cli "$BIN"
codesign -d -r- "$BIN" 2>&1 | grep designated
