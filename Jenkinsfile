library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
def RUST_IMAGE_TAG = '1.42'

pipeline {
    agent any
    options {
        timestamps()
        ansiColor 'xterm'
    }
    triggers {
        cron(env.BRANCH_NAME ==~ /\d\.\d/ ? 'H H 1,15 * *' : '')
    }
    stages {
        stage('Test') {
            steps {
                sh """
                    make lint RUST_IMAGE_REPO=${RUST_IMAGE_REPO} RUST_IMAGE_TAG=${RUST_IMAGE_TAG}
                    make test RUST_IMAGE_REPO=${RUST_IMAGE_REPO} RUST_IMAGE_TAG=${RUST_IMAGE_TAG}
                """
            }
            post {
                success {
                    sh "make clean RUST_IMAGE_REPO=${RUST_IMAGE_REPO} RUST_IMAGE_TAG=${RUST_IMAGE_TAG}"
                }
            }
        }
        stage('Build & Publish Images') {
            stages {
                stage('Build Image') {
                    steps {
                        sh "make build-image RUST_IMAGE_REPO=${RUST_IMAGE_REPO} RUST_IMAGE_TAG=${RUST_IMAGE_TAG}"
                    }
                }
                stage('Check Publish Images') {
                    when {
                        branch pattern: "\\d\\.\\d", comparator: "REGEXP"
                    }
                    stages {
                        stage('Publish Images') {
                            input {
                                message "Should we publish the versioned image?"
                                ok "Publish image"
                            }
                            steps {
                                script {
                                    withRegistry('https://docker.io', 'dockerhub-username-password') {
                                        withRegistry('https://icr.io', 'icr-username-password') {
                                            sh 'make publish'
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            post {
                always {
                    sh 'make clean-all'
                }
            }
        }
    }
}
