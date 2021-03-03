library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def TRIGGER_PATTERN = '.*@logdnabot.*'
def publishImage = false

pipeline {
    agent any
    options {
        timestamps()
        ansiColor 'xterm'
    }
    triggers {
        issueCommentTrigger(TRIGGER_PATTERN)
        cron(env.BRANCH_NAME ==~ /\d\.\d/ ? 'H H 1,15 * *' : '')
    }
    environment {
        RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
        RUST_IMAGE_TAG = 'buster-1-stable'
        SCCACHE_BUCKET = 'logdna-sccache-us-west-2'
        SCCACHE_REGION = 'us-west-2'
        CARGO_INCREMENTAL = 'false'
    }
    stages {
        stage('Validate PR Source') {
          when {
            expression { env.CHANGE_FORK }
            not {
                triggeredBy 'issueCommentCause'
            }
          }
          steps {
            error("A maintainer needs to approve this PR for CI by commenting")
          }
        }
        stage('Pull Build Image') {
            steps {
                sh "docker pull ${RUST_IMAGE_REPO}:${RUST_IMAGE_TAG}"
            }
        }
        stage('Lint and Test') {
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }
            parallel {
                stage('Lint, Unit and Integration Tests'){
                    steps {
                        script {
                            def creds = readJSON file: CREDS_FILE
                            // Assumes the pipeline-e2e-creds format remains the same. Chase
                            // refer to the e2e tests's README's authorization docs for the
                            // current structure
                            LOGDNA_INGESTION_KEY = creds["packet-stage"]["account"]["ingestionkey"]
                        }
                        withCredentials([[
                                                 $class: 'AmazonWebServicesCredentialsBinding',
                                                 credentialsId: 'aws',
                                                 accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                                 secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                         ]]){
                            sh """
                        make lint
                        make test
                        make integration-test LOGDNA_INGESTION_KEY=${LOGDNA_INGESTION_KEY}
                    """
                        }
                    }
                    post {
                        success {
                            sh "make clean"
                        }
                    }
                }
                stage('Run K8s Integration Tests') {
                    steps {
                        withCredentials([[
                                                 $class: 'AmazonWebServicesCredentialsBinding',
                                                 credentialsId: 'aws',
                                                 accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                                 secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                         ]]) {
                            sh script: '''
                                    echo Running k8s integration tests
                                    # Install kind, it should be moved to the image
                                    if ! command -v kind &> /dev/null
                                    then
                                        echo "Installing kind"
                                        curl -Lo ./kind https://kind.sigs.k8s.io/dl/v0.10.0/kind-linux-amd64
                                        chmod +x ./kind
                                        sudo mv ./kind /usr/local/bin/
                                        kind --version
                                    fi
                                    
                                    if ! command -v kubectl &> /dev/null
                                    then
                                        echo "Installing kubectl"
                                        curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
                                        chmod +x ./kubectl
                                        sudo mv ./kubectl /usr/local/bin/
                                        kubectl version --short=true || true
                                    fi
                            ''', label: 'Install tools'

                            sh '''
                                    make k8s-test
                            '''
                        }
                    }
                }
            }
        }
        stage('Build Release Image') {
            steps {
                withCredentials([[
                    $class: 'AmazonWebServicesCredentialsBinding',
                    credentialsId: 'aws',
                    accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                    secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                ]]){
                    sh """
                        echo "[default]" > ${PWD}/.aws_creds
                        echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${PWD}/.aws_creds
                        echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${PWD}/.aws_creds
                        make build-image AWS_SHARED_CREDENTIALS_FILE=${PWD}/.aws_creds
                    """
                }
            }
            post {
                always {
                    sh "rm ${PWD}/.aws_creds"
                }
            }
        }
        stage('Check Publish Images') {
            when {
                branch pattern: "\\d\\.\\d.*", comparator: "REGEXP"
            }
            stages {
                stage('Check Publish Image or Timeout') {
                    steps {
                        script {
                            publishImage = true
                            try {
                                timeout(time: 5, unit: 'MINUTES') {
                                    input(message: 'Should we publish the versioned image?')
                                }
                            } catch (err) {
                                publishImage = false
                            }
                        }
                    }
                }
                stage('Publish Images') {
                    when {
                        expression { return publishImage == true }
                    }
                    steps {
                        script {
                            withCredentials([[
                                $class: 'AmazonWebServicesCredentialsBinding',
                                credentialsId: 'aws',
                                accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                            ]]){
                                sh 'make publish-image'
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
