library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

def boolean changesMatch(String pathRegex) {
    return !env.CHANGE_TARGET || sh(
        script: "git diff --name-only origin/${env.CHANGE_TARGET}...${env.GIT_COMMIT} | grep \"${pathRegex}\"",
        returnStatus: true
    ) == 0
}

def RUST_IMAGE = 'docker.pkg.github.com/answerbook/docker-images/logdna-agent-rust:latest'

pipeline {
    agent any
    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Build Rust Image') {
            when {
                anyOf {
                    expression {
                        return changesMatch('^Makefile$')
                    }
                    expression {
                        return changesMatch('^rust-image/')
                    }
                }
            }
            steps {
                sh 'make -f Makefile.docker rust-image'
                script {
                    RUST_IMAGE = sh(
                        script: 'make -f Makefile.docker get-rust-image',
                        returnStdout: true
                    )
                }
            }
        }
        stage('Test') {
            steps {
                sh "make -f Makefile.docker test IMAGE=${RUST_IMAGE}"
            }
            post {
                success {
                    sh "make -f Makefile.docker clean IMAGE=${RUST_IMAGE}"
                }
            }
        }
        stage('Build & Publish Images') {
            stages {
                stage('Build Image') {
                    steps {
                        sh "make -f Makefile.docker build-image PULL=0 IMAGE=${RUST_IMAGE}"
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
                                    expression {
                                        return changesMatch('^Makefile$')
                                    }
                                    expression {
                                        return changesMatch('^rust-image/')
                                    }
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
