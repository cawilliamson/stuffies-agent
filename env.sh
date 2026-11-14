# sa development environment. source it, do not execute it:
#
#   . ./env.sh
#
# every variable below is read by the daemon process, not by the client. start the daemon
# from the same shell so it inherits them.

KEY_FILE=/run/agenix/clankers-api-key
if [ -r "$KEY_FILE" ]; then
  OPENAI_API_TOKEN=$(sed -E 's/^VLLM_API_KEY=//' "$KEY_FILE")
  export OPENAI_API_TOKEN
else
  echo "env.sh: cannot read $KEY_FILE, turns will fail" >&2
fi

export OPENAI_API_ENDPOINT=https://clankers.chrisaw.io/v1
export SA_MODEL=deepseek-v4-1-flash-1017-ablit
export SA_CONTEXT_LIMIT=1048576
export SA_REASONING_EFFORT=low

echo "sa env: ${SA_MODEL:-unset} @ ${OPENAI_API_ENDPOINT:-unset}"
echo "  ./target/debug/sa daemon &"
echo "  cd /some/repo && ../sa attach"
