library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'

pipeline {
    agent any
    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Test') {
            steps {
                sh "make -f Makefile.docker test IMAGE_REPO=${RUST_IMAGE_REPO}"
            }
            post {
                success {
                    sh "make -f Makefile.docker clean IMAGE_REPO=${RUST_IMAGE_REPO}"
                }
            }
        }
        stage('Build & Publish Images') {
            stages {
                stage('Build Image') {
                    steps {
                        sh "make -f Makefile.docker build-image PULL=0 IMAGE_REPO=${RUST_IMAGE_REPO}"
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
}
