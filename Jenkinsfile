library 'magic-butler-catalogue'
def PROJECT_NAME = 'logdna-agent-v2'
def TRIGGER_PATTERN = '.*@logdnabot.*'
def publishImage = false
def runBenchmarks = false

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
        parallel {
            stage ("Lint and Test"){
                environment {
                    CREDS_FILE = credentials('pipeline-e2e-creds')
                    LOGDNA_HOST = "logs.use.stage.logdna.net"
                }
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
            stage('Run performance regression testing') {
                when {
                    not {
                        branch 'master'
                    }
                }
                stages {
                    stage('Manual approval for regression testing') {
                        steps {
                            script {
                                runBenchmarks = true
                                try {
                                    timeout(time: 30, unit: 'SECONDS') {
                                        input(message: 'Run optional performance regression tests?')
                                    }
                                } catch (err) {
                                    runBenchmarks = false
                                }
                            }
                        }
                    }
                    stage('Run benchmarks') {
                        when {
                            expression { return runBenchmarks == true }
                        }
                        steps {
                            script {
                                withCredentials([[
                                                         $class: 'AmazonWebServicesCredentialsBinding',
                                                         credentialsId: 'aws',
                                                         accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                                         secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                                 ]]){
                                    timeout(time: 15, unit: 'MINUTES') {
                                        sh script: '''
                                        echo "Running performance benchmarks for $GIT_BRANCH with master as baseline"
                                        rm -Rf agent-benchmarks
                                        git clone -q https://github.com/logdna/agent-benchmarks.git && cd agent-benchmarks
                                        echo "" > dummy_github_id_rsa
                                        terraform init ./terraform/
                                        terraform apply -auto-approve -var="baseline_agent_type=rust" -var="compare_agent_type=rust" -var="compare_agent_branch=$GIT_BRANCH" -var="test_scenario=2" -var="path_to_ssh_key=dummy_github_id_rsa" -var="aws_access_key=$AWS_ACCESS_KEY_ID" -var="aws_secret_key=$AWS_SECRET_ACCESS_KEY" -var="bucket_folder=$GIT_BRANCH-$BUILD_NUMBER" ./terraform/
                                    ''', label: 'Run benchmarks using terraform'
                                    }

                                    echo "Memory usage: https://agent-benchmarks-results.s3.us-east-2.amazonaws.com/${env.GIT_BRANCH}-${env.BUILD_NUMBER}/memory-series.png"
                                    echo "CPU histogram (this branch): https://agent-benchmarks-results.s3.us-east-2.amazonaws.com/${env.GIT_BRANCH}-${env.BUILD_NUMBER}/benchmarks/compare/cpu.txt"
                                    echo "CPU histogram (baseline): https://agent-benchmarks-results.s3.us-east-2.amazonaws.com/${env.GIT_BRANCH}-${env.BUILD_NUMBER}/benchmarks/baseline/cpu.txt"
                                }
                            }
                        }
                        post {
                            always {
                                withCredentials([[
                                                         $class: 'AmazonWebServicesCredentialsBinding',
                                                         credentialsId: 'aws',
                                                         accessKeyVariable: 'AWS_ACCESS_KEY_ID',
                                                         secretKeyVariable: 'AWS_SECRET_ACCESS_KEY'
                                                 ]]){
                                    retry(3) {
                                        sh script: '''
                                        cd agent-benchmarks
                                        terraform destroy -auto-approve ./terraform/
                                    ''', label: 'Destroy resources using terraform'
                                    }
                                }
                            }
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
