case "$LAUNCHER_ITEM" in
  "Show date") date ;;
  "Show system information")
    uname -a
    printf '\n'
    id
    ;;
esac
