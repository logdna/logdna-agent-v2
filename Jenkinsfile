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
        parallel {
            stage('Prepare Test Environment') {
                steps {
                    sh 'make test-deps'
                }
            }
            stage('Build') {
                steps {
                    sh 'make build'
                }
            }
        }
        stage('Test') {
            steps {
                sh 'make test'
            }
        }
        stage('Deploy to Dockerhub') {
            steps {
                def buildImage = docker.build("logdna-agent:stable")
            }
        }
    }
}
