library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def TRIGGER_PATTERN = '.*@logdnabot.*'
def publishGCRImage = false
def publishDockerhubICRImages = false

pipeline {
    agent {
        node {
            label "rust-x86_64"
            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
        }
    }
    options {
        timeout time: 8, unit: 'HOURS'
        timestamps()
        ansiColor 'xterm'
    }
    triggers {
        issueCommentTrigger(TRIGGER_PATTERN)
        parameterizedCron(
            env.BRANCH_NAME ==~ /\d\.\d/ ? 'H 8 * * 1 % PUBLISH_GCR_IMAGE=true;PUBLISH_ICR_IMAGE=true' : ''
        )
    }
    environment {
        RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
        RUST_IMAGE_TAG = 'bullseye-1-stable'
        TOOLS_IMAGE_TAG = 'bullseye-1-stable'
        SCCACHE_BUCKET = 'logdna-sccache-us-west-2'
        SCCACHE_REGION = 'us-west-2'
        CARGO_INCREMENTAL = 'false'
        DOCKER_BUILDKIT = '1'
    }
    parameters {
        booleanParam(name: 'PUBLISH_GCR_IMAGE', description: 'Publish docker image to Google Container Registry (GCR)', defaultValue: false)
        booleanParam(name: 'PUBLISH_ICR_IMAGE', description: 'Publish docker image to IBM Container Registry (ICR) and Dockerhub', defaultValue: false)
        booleanParam(name: 'PUBLISH_BINARIES', description: 'Publish executable binaries to S3 bucket s3://logdna-agent-build-bin', defaultValue: false)
        booleanParam(name: 'PUBLISH_INSTALLERS', description: 'Publish Choco installer', defaultValue: false)
        string(name: 'RUST_IMAGE_SUFFIX', description: 'Build image tag suffix', defaultValue: "")
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
        stage('Init QEMU') {
            steps {
                sh "make init-qemu"
            }
        }
        stage('Vendor') {
            steps {
                sh """
                    mkdir -p .cargo || /bin/true
                    make vendor
                """
            }
        }
        stage('Build Release Binaries') {
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }
            parallel {           
                stage('Build Mac OSX release binary X86_64') {
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}/x86")
                        }
                    }
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh '''
                                source $HOME/.cargo/env
                                source ~/.bash_profile
                                echo "[default]" > ${WORKSPACE}/.aws_creds_mac_static_x86_64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_mac_static_x86_64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_mac_static_x86_64
                                cargo build --release --target=x86_64-apple-darwin --target-dir x86-target
                                rm ${WORKSPACE}/.aws_creds_mac_static_x86_64
                            '''
                        }
                    }
                }
                stage('Build Mac OSX release binary ARM64') {
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}/arm")
                        }
                    }
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh '''
                                source $HOME/.cargo/env
                                source ~/.bash_profile
                                echo "[default]" > ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                cargo build --release --target-dir arm-target
                                rm ${WORKSPACE}/.aws_creds_mac_static_arm64
                            '''
                        }
                    }
                }
            }
        }
        stage('Check Publish Images') {
            stages {
                stage('Publish MAC binaries to S3') {
                    agent {
                        node {
                            label "osx-node"
                            customWorkspace("/tmp/workspace/${env.BUILD_TAG}")
                        }
                    }
                    steps {
                        withCredentials([[
                            $class: 'AmazonWebServicesCredentialsBinding',
                            credentialsId: 'aws',
                            accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                            secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                        ]]) {
                            sh """
                                source $HOME/.cargo/env
                                source ~/.bash_profile
                                echo "[default]" > ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                echo "AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY}" >> ${WORKSPACE}/.aws_creds_mac_static_arm64
                                rm ${WORKSPACE}/.aws_creds_mac_static_arm64
                                rm -r ${WORSKPACE}
                            """
                        }
                    }
                }
            }
        }
    }
}
