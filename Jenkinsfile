library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent {
        docker {
            image 'rust:1.41'
        }
    }
    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Prepare Environment') {
            steps {
                sh 'rustup update'
                sh 'rustup toolchain install nightly'
                sh 'rustup component add clippy'
                sh 'rustup component add rustfmt'
                sh 'cargo +nightly install cargo-udeps --locked'
            }
        }
        stage('Build') {
            steps {
                sh 'make build'
            }
        }
        stage('Test') {
            steps {
                sh 'make test'
            }
        }
    }
}
