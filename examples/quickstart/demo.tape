# Renders docs/assets/quickstart.gif. Needs vhs (https://github.com/charmbracelet/vhs):
#   cd examples/quickstart && npm install && vhs demo.tape

Output ../../docs/assets/quickstart.gif

Set Shell "bash"
Set FontSize 20
Set Width 1200
Set Height 760
Set Padding 28
Set Theme "Catppuccin Mocha"
Set TypingSpeed 35ms

Hide
Type "export PS1='$ ' && rm -rf .used paused.json .secret && clear"
Enter
Show

Type "# 1. Run the agent's script. The refund needs approval."
Enter
Type "node refund.mjs"
Sleep 400ms
Enter
Sleep 4s

Type "# 2. A new process approves. Nothing runs twice."
Enter
Type "node refund.mjs approve"
Sleep 400ms
Enter
Sleep 4s

Type "# 3. The same token again?"
Enter
Type "node refund.mjs approve"
Sleep 400ms
Enter
Sleep 4s
