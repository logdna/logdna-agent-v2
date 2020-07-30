library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent any

    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Build Rust Image') {
            when {
                changeset "Makefile", "rust-image/**/*"
            }
            steps {
                sh 'make -f Makefile.docker rust-image'
            }
        }
        stage('Test') {
            steps {
                sh 'make -f Makefile.docker test'
            }
            post {
                success {
                    sh 'make -f Makefile.docker clean'
                }
            }
        }
        stage('Build & Publish Images') {
            stages {
                stage('Build Image') {
                    steps {
                        sh 'make -f Makefile.docker build-image'
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
                                changeset "Makefile", "rust-image/**/*"
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
            script {
                println(currentBuild.changeSets)
            }
        }
    }
}
