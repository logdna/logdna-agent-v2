library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent any
    options {
        timestamps()
        ansiColor 'xterm'
    }
    environment {
        RUST_IMAGE = 'docker.pkg.github.com/answerbook/docker-images/logdna-agent-rust:latest'
    }
    stages {
        stage('Build Rust Image') {
            when {
                anyOf {
                    changeset "Makefile"
                    changeset "rust-image/**/*"
                }
            }
            steps {
                sh 'make -f Makefile.docker rust-image'
                script {
                    env.RUST_IMAGE_TAG = sh 'make -f Makefile.docker get-rust-image'
                }
            }
        }
        stage('Test') {
            steps {
                sh 'make -f Makefile.docker test IMAGE=${RUST_IMAGE}'
            }
            post {
                success {
                    sh 'make -f Makefile.docker clean IMAGE=${RUST_IMAGE}'
                }
            }
        }
        stage('Build & Publish Images') {
            stages {
                stage('Build Image') {
                    steps {
                        sh 'make -f Makefile.docker build-image IMAGE=${RUST_IMAGE}'
                    }
                }
                stage('Publish Images') {
                    parallel {
                        stage('Publish Public Images') {
                            when {
                                branch pattern: "\\d\\.\\d", comparator: "REGEXP"
                            }
                            steps {
                                sh 'make -f Makefile.docker publish-public'
                            }
                        }
                        stage('Publish Private Images') {
                            when {
                                branch 'master'
                            }
                            steps {
                                sh 'make -f Makefile.docker publish-private'
                            }
                        }
                        stage('Publish CI Rust Image') {
                            when {
                                branch 'master'
                                anyOf {
                                    changeset "Makefile"
                                    changeset "rust-image/**/*"
                                }
                            }
                            steps {
                                sh 'make -f Makefile.docker publish-rust'
                            }
                        }
                    }
                }
            }
            post {
                always {
                    sh 'make -f Makefile.docker clean-images'
                }
            }
        }
    }
    post {
        always {
            sh 'make -f Makefile.docker clean-rust-image'
        }
    }
}
