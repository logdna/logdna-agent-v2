library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def TRIGGER_PATTERN = '.*@logdnabot.*'
def slugify(str) {
  def s = str.toLowerCase()
  s = s.replaceAll(/[^a-z0-9\s-\/]/, "").replaceAll(/\s+/, " ").trim()
  s = s.replaceAll(/[\/\s]/, '-').replaceAll(/-{2,}/, '-')
  s
}

def withAwsCreds(Closure body) {
    withCredentials([[
        $class: 'AmazonWebServicesCredentialsBinding',
        credentialsId: 'aws'
    ]]) {
        def credsFile = "${env.WORKSPACE}/.aws_creds_${UUID.randomUUID()}"

        withEnv(["AWS_SHARED_CREDENTIALS_FILE=${credsFile}"]) {
            sh '''
                echo "[default]" > $AWS_SHARED_CREDENTIALS_FILE
                echo "AWS_ACCESS_KEY_ID=$AWS_ACCESS_KEY_ID" >> $AWS_SHARED_CREDENTIALS_FILE
                echo "AWS_SECRET_ACCESS_KEY=$AWS_SECRET_ACCESS_KEY" >> $AWS_SHARED_CREDENTIALS_FILE
            '''

            try {
                body.call()
            } finally {
                sh 'rm -f $AWS_SHARED_CREDENTIALS_FILE'
            }
        }
    }
}

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
        parameterizedCron(env.BRANCH_NAME ==~ /\d+\.\d+/ ? '''
            H 8 * * 1 % BUILD_ALL=false;PUBLISH_GCR_IMAGE=true;PUBLISH_DOCKER_IMAGE=true;TASK_NAME=image-vulnerability-update
            H 12 * * 1 % BUILD_ALL=false;AUDIT=true;TASK_NAME=audit
            ''' : ''
        )
    }
    environment {
        RUST_IMAGE_REPO = 'us.gcr.io/logdna-k8s/rust'
        RUST_IMAGE_TAG = 'bookworm-1-stable'
        TOOLS_IMAGE_TAG = 'bookworm-1-stable'
        SCCACHE_BUCKET = 'logdna-sccache-us-west-2'
        SCCACHE_REGION = 'us-west-2'
        CARGO_INCREMENTAL = 'false'
        DOCKER_BUILDKIT = '1'
        REMOTE_BRANCH = "${env.BRANCH_NAME}"
        BUILD_SLUG = slugify("${BUILD_TAG}")
    }
    parameters {
        booleanParam(name: 'BUILD_ALL', description: 'Build all artifacts regardless of publish parameters', defaultValue: true)
        booleanParam(name: 'PUBLISH_GCR_IMAGE', description: 'Publish docker image to Google Container Registry (GCR)', defaultValue: false)
        booleanParam(name: 'PUBLISH_DOCKER_IMAGE', description: 'Publish docker image to Dockerhub', defaultValue: false)
        booleanParam(name: 'PUBLISH_LINUX_BINARIES', description: 'Publish linux executable binaries to S3 bucket s3://logdna-agent-build-bin', defaultValue: false)
        booleanParam(name: 'PUBLISH_WINDOWS_BINARIES', description: 'Publish windows executable binaries to S3 bucket s3://logdna-agent-build-bin', defaultValue: false)
        booleanParam(name: 'PUBLISH_CHOCO_INSTALLER', description: 'Publish Choco installer', defaultValue: false)
        booleanParam(name: 'AUDIT', description: 'Check for application vulnerabilities with cargo audit', defaultValue: false)
        string(name: 'RUST_IMAGE_SUFFIX', description: 'Build image tag suffix', defaultValue: "")
        choice(name: 'TASK_NAME', choices: ['n/a', 'audit', 'image-vulnerability-update'], description: 'The name of the task being handled in this build, if applicable')
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
        stage('Build Release') {
            environment {
                CREDS_FILE = credentials('pipeline-e2e-creds')
                LOGDNA_HOST = "logs.use.stage.logdna.net"
            }
            parallel {
                stage('Build Release Image x86_64') {
                    when {
                        anyOf {
                            environment name: 'BUILD_ALL', value: 'true'
                            environment name: 'PUBLISH_GCR_IMAGE', value: 'true'
                            environment name: 'PUBLISH_DOCKER_IMAGE', value: 'true'
                        }
                    }
                    environment {
                        ARCH = 'x86_64'
                    }
                    steps {
                        script {
                            withAwsCreds {
                                sh 'make build-image'
                            }
                        }
                    }
                }
                stage('Build Release Image aarch64') {
                    when {
                        anyOf {
                            environment name: 'BUILD_ALL', value: 'true'
                            environment name: 'PUBLISH_GCR_IMAGE', value: 'true'
                            environment name: 'PUBLISH_DOCKER_IMAGE', value: 'true'
                        }
                    }
                    environment {
                        ARCH = 'aarch64'
                    }
                    steps {
                        script {
                            withAwsCreds {
                                sh 'make build-image'
                            }
                        }
                    }
                }
                stage('Build static release binary x86_64') {
                    when {
                        anyOf {
                            environment name: 'BUILD_ALL', value: 'true'
                            environment name: 'PUBLISH_LINUX_BINARIES', value: 'true'
                        }
                    }
                    environment {
                        ARCH = 'x86_64'
                        STATIC = '1'
                    }
                    steps {
                        sh 'make build-release'
                    }
                }
                stage('Build static release binary aarch64') {
                    when {
                        anyOf {
                            environment name: 'BUILD_ALL', value: 'true'
                            environment name: 'PUBLISH_LINUX_BINARIES', value: 'true'
                        }
                    }
                    environment {
                        ARCH = 'aarch64'
                        STATIC = '1'
                    }
                    steps {
                        sh 'make build-release'
                    }
                }
                stage('Build Windows release binary x86_64') {
                    when {
                        anyOf {
                            environment name: 'BUILD_ALL', value: 'true'
                            environment name: 'PUBLISH_WINDOWS_BINARIES', value: 'true'
                            environment name: 'PUBLISH_CHOCO_INSTALLER', value: 'true'
                        }
                    }
                    environment {
                        ARCH = 'x86_64'
                        WINDOWS = '1'
                        FEATURES = 'windows_service'
                    }
                    steps {
                        sh 'make build-release'
                    }
                }
            }
            post {
                always {
                    sh 'make clean'
                }
            }
        }
        stage('Check Publish Images') {
            stages {
                stage('Scanning Images') {
                    when {
                        anyOf {
                            environment name: 'PUBLISH_GCR_IMAGE', value: 'true'
                            environment name: 'PUBLISH_DOCKER_IMAGE', value: 'true'
                        }
                    }
                    steps {
                        sh 'ARCH=x86_64 make sysdig_secure_images'
                        script {
                            def imageNames = readFile('sysdig_secure_images').trim().split('\n')
                            for(img in imageNames){
                                echo "Scanning image ${img}"
                                sysdigImageScan engineCredentialsId: 'sysdig-secure-api-token', imageName: img
                            }
                        }
                        sh 'ARCH=aarch64 make sysdig_secure_images'
                        script {
                            def imageNames = readFile('sysdig_secure_images').trim().split('\n')
                            for(img in imageNames){
                                echo "Scanning image ${img}"
                                sysdigImageScan engineCredentialsId: 'sysdig-secure-api-token', imageName: img
                            }
                        }
                    }
                }
                stage('Publish GCR images') {
                    when {
                        environment name: 'PUBLISH_GCR_IMAGE', value: 'true'
                    }
                    steps {
                        // Publish to gcr, jenkins is logged into gcr globally
                        sh 'ARCH=x86_64 make publish-image-gcr'
                        sh 'ARCH=aarch64 make publish-image-gcr'
                        sh 'make publish-image-multi-gcr'
                    }
                }
                stage('Publish Dockerhub') {
                    when {
                        environment name: 'PUBLISH_DOCKER_IMAGE', value: 'true'
                    }
                    steps {
                        script {
                            // Login and publish to dockerhub
                            docker.withRegistry(
                                'https://index.docker.io/v1/',
                                'dockerhub-username-password'
                            ) {
                                sh 'ARCH=x86_64 make publish-image-docker'
                                sh 'ARCH=aarch64 make publish-image-docker'
                                sh 'make publish-image-multi-docker'
                            }
                        }
                    }
                }
                stage('Publish Linux Binaries') {
                    when {
                        environment name: 'PUBLISH_LINUX_BINARIES', value: 'true'
                    }
                    steps {
                        sh '''
                            ARCH=x86_64 STATIC=1 make publish-s3-binary
                            ARCH=aarch64 STATIC=1 make publish-s3-binary
                        '''
                    }
                }

                stage('Publish Windows Binaries') {
                    when {
                        environment name: 'PUBLISH_WINDOWS_BINARIES', value: 'true'
                    }
                    environment {
                        WINDOWS = '1'
                    }
                    steps {
                        sh '''
                            make publish-s3-binary
                            make msi-release
                            make test-msi-release
                            make publish-s3-binary-signed-release
                        '''
                    }
                }

                stage('Publish Choco Installer') {
                    when {
                        environment name: 'PUBLISH_CHOCO_INSTALLER', value: 'true'
                    }
                    environment {
                        CHOCO_API_KEY = credentials('chocolatey-api-token')
                        CSC_PASS = credentials('chocolatey-api-token')
                        WINDOWS = '1'
                    }
                    steps {
                        sh '''
                            make choco-release
                            make publish-s3-choco-release
                            make publish-choco-release
                        '''
                    }
                }

            }
        }
    }
    post {
        success {
            script {
                if (params.TASK_NAME == 'image-vulnerability-update') {
                    //TODO: change channel to #ibm-mezmo-agent after testing
                    notifySlack(
                        currentBuild.currentResult,
                        [channel: '#proj-agent'],
                        "`${PROJECT_NAME}` ${params.TASK_NAME} build took ${currentBuild.durationString.replaceFirst(' and counting', '')}."
                    )
                }
            }
        }
        unsuccessful {
            script {
                if (params.TASK_NAME == 'audit' || params.TASK_NAME == 'image-vulnerability-update') {
                    notifySlack(
                        currentBuild.currentResult,
                        [channel: '#proj-agent'],
                        "`${PROJECT_NAME}` ${params.TASK_NAME} build took ${currentBuild.durationString.replaceFirst(' and counting', '')}."
                    )
                }
            }
        }
        always {
            sh """
                ARCH=x86_64 make clean-all
                ARCH=aarch64 make clean-all
            """
            cleanWs(deleteDirs: true,
                    // Uncomment the 'cleanWhenFailure: false,' line for debug
                    // cleanWhenFailure: false,
                    notFailBuild: true,
                    patterns: [[pattern: '.gitignore', type: 'INCLUDE'],
                                [pattern: '.propsfile', type: 'EXCLUDE']])
        }
    }
}
