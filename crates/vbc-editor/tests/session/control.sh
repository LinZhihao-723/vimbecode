#!/bin/sh
# A stand-in for the `claude` binary that asks questions and waits for the answers.
#
# The other stand-in answers turns. This one is about the round trips a turn stops for, and it
# exists because the real binary cannot be made to produce most of them on demand: two questions
# outstanding at once is a thing claude 2.1.263 will not do -- it asks about one parallel tool call,
# waits, and asks about the next only once the first is answered -- and a client whose queue was
# only ever tested against that would be a client that has never had two questions in it.
#
# Everything it does is a mirror of something measured against claude 2.1.263, so the tests reading
# it are reading that behaviour rather than a convenient one:
#
#   * a question is a `control_request` with the subtype `can_use_tool`, and until it is answered
#     the session writes nothing else at all -- no prose, no result, no error
#   * `AskUserQuestion` carries `requires_user_interaction`, and an approval that carries no
#     `answers` in its `updatedInput` resolves as the reader having declined to answer, which the
#     real binary reports in those words
#   * a denied call is named in the result frame's `permission_denials` and nowhere else
#   * an interrupt asked to keep the queue writes a receipt naming what is still queued, and then
#     starts the next queued turn; asked to cancel the queue it names what it threw away as well,
#     and nothing follows
#
# What it does is chosen by a file in the directory it is started in, which is the one thing about
# a spawn a test can choose through the client's own interface:
#
#   two         ask about two calls at once, so that they can be answered in either order
#   question    ask a question rather than for an approval
#   plan        ask for a plan to be approved, and announce the new mode once it is
#   other       make a control request that is not a question at all
#   auto        answer the turn with no question at all, as an auto-approved call does
#   interrupt   start a turn and never end it, and queue what arrives until it is stopped
#
# With none of them it asks about one write. It writes down what it was answered in `answered`,
# one request and behaviour per line and in the order the answers arrived, and what an interrupt
# asked of the queue in `cancel`.

printf '%s\n' "$@" > args

session=""
resume=""
while [ $# -gt 0 ]; do
    case "$1" in
        --session-id) session="$2"; shift 2 ;;
        --resume) resume="$2"; shift 2 ;;
        *) shift ;;
    esac
done

sid="$session"
if [ -z "$sid" ]; then
    sid="$resume"
fi

here=$(pwd)
tools='"Bash","Read","Edit","Write","AskUserQuestion","EnterPlanMode","ExitPlanMode"'

mode="write"
for candidate in two question plan other auto interrupt; do
    if [ -f "$candidate" ]; then
        mode="$candidate"
    fi
done

# The value of a string field of a JSON line, which every field these lines carry is unique in.
value() {
    printf '%s' "$2" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"
}

announce() {
    printf '{"type":"system","subtype":"init","session_id":"%s","claude_code_version":"stub",' "$sid"
    printf '"model":"stub","cwd":"%s","permissionMode":"default","tools":[%s],' "$here" "$tools"
    printf '"capabilities":[],"slash_commands":[]}\n'
}

say() {
    printf '{"type":"assistant","session_id":"%s","parent_tool_use_id":null,' "$sid"
    printf '"message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\n' "$1"
}

end() {
    printf '{"type":"result","subtype":"%s","session_id":"%s","is_error":false,' "$1" "$sid"
    printf '"num_turns":%s,"result":"%s","permission_denials":[%s]}\n' "$turn" "$2" "$denials"
}

ask() {
    printf '{"type":"control_request","request_id":"%s","request":{"subtype":"can_use_tool",' "$1"
    printf '"tool_name":"%s","display_name":"%s","description":"a call the stub made",' "$2" "$2"
    printf '"tool_use_id":"toolu-%s","input":%s%s}}\n' "$1" "$3" "$4"
}

turn=0
outstanding=0
queued=0
running="no"
denials=""
answered=""

while IFS= read -r line; do
    case "$line" in
        *'"type":"user"'*)
            if [ "$running" = "yes" ]; then
                queued=$((queued + 1))
                continue
            fi

            turn=$((turn + 1))
            denials=""
            announce
            case "$mode" in
                auto)
                    say "no question was asked"
                    end "success" "no question was asked"
                    ;;
                other)
                    printf '{"type":"control_request","request_id":"req-hook",'
                    printf '"request":{"subtype":"hook_callback","callback_id":"hook-1"}}\n'
                    say "nothing was asked of the reader"
                    end "success" "nothing was asked of the reader"
                    ;;
                plan)
                    ask "req-plan" "ExitPlanMode" \
                        '{"plan":"# Plan\n\n- read the file\n- write it back"}' \
                        ',"requires_user_interaction":true'
                    outstanding=1
                    ;;
                two)
                    ask "req-first" "Write" '{"file_path":"first.txt","content":"first"}' ""
                    ask "req-second" "Write" '{"file_path":"second.txt","content":"second"}' ""
                    outstanding=2
                    ;;
                question)
                    ask "req-question" "AskUserQuestion" \
                        '{"questions":[{"question":"Tea or coffee?","header":"Drink","options":[{"label":"Tea","description":"a pot of it"},{"label":"Coffee","description":"a cup of it"}],"multiSelect":false}]}' \
                        ',"requires_user_interaction":true'
                    outstanding=1
                    ;;
                interrupt)
                    running="yes"
                    say "working on it"
                    ;;
                *)
                    ask "req-write" "Write" '{"file_path":"note.txt","content":"hello"}' ""
                    outstanding=1
                    ;;
            esac
            ;;

        *'"type":"control_response"'*)
            request=$(value "request_id" "$line")
            behaviour=$(value "behavior" "$line")
            printf '%s %s\n' "$request" "$behaviour" >> answered
            answered="$answered $request"

            if [ "$behaviour" = "allow" ]; then
                case "$mode" in
                    plan)
                        printf '{"type":"system","subtype":"status","session_id":"%s",' "$sid"
                        printf '"status":null,"permissionMode":"default"}\n'
                        answer="the plan was approved"
                        ;;
                    question)
                        case "$line" in
                            *'"answers":'*)
                                chosen=$(printf '%s' "$line" \
                                    | sed -n 's/.*"answers":{"[^"]*":"\([^"]*\)".*/\1/p')
                                printf '{"type":"user","session_id":"%s",' "$sid"
                                printf '"message":{"role":"user","content":[{"type":"tool_result",'
                                printf '"tool_use_id":"toolu-%s","content":' "$request"
                                printf '"The user answered: \\"Tea or coffee?\\"=\\"%s\\"."}]}}\n' \
                                    "$chosen"
                                answer="answered:$chosen"
                                ;;
                            *)
                                printf '{"type":"user","session_id":"%s",' "$sid"
                                printf '"message":{"role":"user","content":[{"type":"tool_result",'
                                printf '"tool_use_id":"toolu-%s","content":' "$request"
                                printf '"The user did not answer the questions."}]}}\n'
                                answer="unanswered"
                                ;;
                        esac
                        ;;
                    *)
                        path=$(value "file_path" "$line")
                        content=$(value "content" "$line")
                        printf '%s\n' "$content" > "$path"
                        answer="written"
                        ;;
                esac
            else
                tool="Write"
                if [ "$mode" = "question" ]; then
                    tool="AskUserQuestion"
                fi
                if [ -n "$denials" ]; then
                    denials="$denials,"
                fi
                denials="$denials{\"tool_name\":\"$tool\",\"tool_use_id\":\"toolu-$request\","
                denials="$denials\"tool_input\":{}}"
                answer="refused"
            fi

            outstanding=$((outstanding - 1))
            if [ "$outstanding" -le 0 ]; then
                say "$answer"
                end "success" "$answer"
            fi
            ;;

        *'"type":"control_request"'*)
            request=$(value "request_id" "$line")
            case "$line" in
                *'"cancel_queued":true'*)
                    printf 'true\n' > cancel
                    printf '{"type":"control_response","response":{"subtype":"success",'
                    printf '"request_id":"%s","response":{"still_queued":[],' "$request"
                    printf '"cancelled":[]}}}\n'
                    queued=0
                    ;;
                *)
                    printf 'false\n' > cancel
                    printf '{"type":"control_response","response":{"subtype":"success",'
                    printf '"request_id":"%s","response":{"still_queued":[]}}}\n' "$request"
                    ;;
            esac

            running="no"
            denials=""
            end "error_during_execution" "the turn was stopped"

            while [ "$queued" -gt 0 ]; do
                queued=$((queued - 1))
                turn=$((turn + 1))
                announce
                say "the queue drained"
                end "success" "the queue drained"
            done
            ;;
    esac
done
