library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'

pipeline {
    agent none
    options {
        timestamps()
        ansiColor 'xterm'
    }
    stages {
        stage('Build and Test') {
            agent {
                docker {
                    image 'rust:1.41'
                }
            }
            stages {
                stage('Build') {
                    parallel {
                        stage('Build Test Dependencies') {
                            steps {
                                sh 'make test-deps'
                            }
                        }
                        stage('Build Agent') {
                            steps {
                                sh 'make build'
                            }
                        }
                    }
                }
                stage('Test') {
                    steps {
                        sh 'make test'
                    }
                }
            }
        }
        stage('Deploy to Dockerhub') {
            agent any
            steps {
                script {
                    docker.withTool("default") {
                        def buildImage = docker.build("logdna-agent:stable")
                    }
                }
            }
        }
    }
}
