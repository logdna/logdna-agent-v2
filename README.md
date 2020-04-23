# LogDNA Agent v2 - Rust Edition

## Overview

The LogDNA agent is log collection client that forwards log lines to LogDNA. This agent supports running both as a local binary as well as a DaemonSet in Kubernetes. Helpful resources can be found below:
* [General information on the logdna agent](https://docs.logdna.com/docs/logdna-agent)
* [The logdna agent public docker image repository](https://hub.docker.com/r/logdna/logdna-agent)
* [Learning Rust](https://doc.rust-lang.org/book/)

## Development

Historically, this agent has been developed in Linux by its maintainer, Connor Peticca. However, there are ongoing efforts to make development compatible with macOS in order to boost the number of contributors.

### macOS

_Warning_: macOS is not yet fully supported for development.

**Requirements:**
* macOS computer with 2+ CPU cores and 8+ GB of RAM
* [Homebrew](https://brew.sh/) is installed
* [authme](https://github.com/answerbook/authme) is installed and working

**Install dependencies:**
```
# Install minikube dependencies (may already be installed)
brew install kubernetes-cli
brew cask install google-cloud-sdk minikube

# Give minikube some muscle
minikube config set cpus 2
minikube config set memory 4096

# Install rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Ensure you can run cargo (rust)
which cargo

# Add rust to your path
echo 'export PATH=$HOME/.cargo/bin::$PATH' >> ~/.bash_profile

# Clone the repo
cd "${LOGDNA_WORKDIR}"
git clone git@github.com:answerbook/logdna-agent-v2
```

**Build the agent:**
```
ldcd logdna-agent-v2

# Check for compile errors - if this passes, your code is valid
cargo check

# Build a release docker image
minikube start
eval $(minikube docker-env) # Use docker inside of minikube
docker build . -t us.gcr.io/logdna-k8s/logdna-agent-v2:latest
```

**Troubleshooting:**

If minikube is already installed and running prior to updating the allocated cpu and/or memory, you will need to stop it and clear it before trying again.
```
minikube stop
minikube delete
```

If you see an error ending in
```
(signal: 9, SIGKILL: kill)
```
you likely do not have enough memory for minikube. You can increase it with
```
minikube config set memory <mem-in-mb>
```

If you receive a compile error, it may be worth running
```
cargo update
```
to determine if the new Cargo.lock file will allow you to compile. If it does, it is worth making a PR with the Cargo.lock file to correct this.

## Linux

Instructions TBD.
