function fail(message) {
    print "LEZ cost parser: " message > "/dev/stderr"
    exit 1
}

function session_value(line, fields, count) {
    sub(/^.*session: /, "", line)
    count = split(line, fields, / +/)
    if (count < 2 || fields[1] !~ /^[0-9]+$/) {
        fail("malformed RISC0 metric: " line)
    }
    return fields[1] + 0
}

BEGIN {
    expected_count = 6
    expected[1] = "initialize_native_claim"
    expected[2] = "fund_native_claim"
    expected[3] = "claim_native"
    expected[4] = "initialize_native_refund"
    expected[5] = "fund_native_refund"
    expected[6] = "refund_native"

    budget["initialize_native_claim"] = 375000
    budget["fund_native_claim"] = 460000
    budget["claim_native"] = 490000
    budget["initialize_native_refund"] = 375000
    budget["fund_native_refund"] = 460000
    budget["refund_native"] = 475000
}

/LEZ_COST_BEGIN / {
    if (active) {
        fail("nested begin marker for " operation)
    }
    marker = $0
    sub(/^.*LEZ_COST_BEGIN /, "", marker)
    operation = marker
    active = 1
    session = 0
    order[++operation_count] = operation
    next
}

/LEZ_COST_END / {
    if (!active) {
        fail("end marker without begin")
    }
    marker = $0
    sub(/^.*LEZ_COST_END /, "", marker)
    if (marker != operation) {
        fail("end marker " marker " does not match " operation)
    }
    if (session != 2) {
        fail(operation " emitted " session " sessions; expected escrow plus chained transfer")
    }
    active = 0
    operation = ""
    next
}

active && /number of segments:/ {
    session++
    segments[operation, session] = $NF + 0
    next
}

active && / total cycles/ {
    if (session == 0) fail("total cycles before session")
    total[operation, session] = session_value($0)
    next
}

active && / user cycles/ {
    if (session == 0) fail("user cycles before session")
    user[operation, session] = session_value($0)
    next
}

active && / paging cycles/ {
    if (session == 0) fail("paging cycles before session")
    paging[operation, session] = session_value($0)
    next
}

active && / reserved cycles/ {
    if (session == 0) fail("reserved cycles before session")
    reserved[operation, session] = session_value($0)
    next
}

END {
    if (active) fail("unterminated operation " operation)
    if (operation_count != expected_count) {
        fail("found " operation_count " operations; expected " expected_count)
    }

    for (i = 1; i <= expected_count; i++) {
        operation = expected[i]
        if (order[i] != operation) {
            fail("operation " i " was " order[i] "; expected " operation)
        }
        for (session = 1; session <= 2; session++) {
            if (segments[operation, session] != 1) {
                fail(operation " session " session " must have exactly one segment")
            }
            if (total[operation, session] == 0 ||
                user[operation, session] == 0 ||
                paging[operation, session] == 0 ||
                reserved[operation, session] == 0) {
                fail(operation " session " session " is missing a metric")
            }
            classified_cycles = user[operation, session] + paging[operation, session] + reserved[operation, session]
            if (total[operation, session] != classified_cycles) {
                fail(operation " session " session " violates total=user+paging+reserved")
            }
        }
        if (total[operation, 1] != 524288 || total[operation, 2] != 131072) {
            fail(operation " allocated-cycle regression")
        }
        recursive_user[operation] = user[operation, 1] + user[operation, 2]
        if (recursive_user[operation] > budget[operation]) {
            fail(operation " exceeds recursive user-cycle budget")
        }
    }

    print "{"
    print "  \"schema_version\": 1,"
    print "  \"measured_on\": \"2026-07-11\","
    print "  \"execution\": {"
    print "    \"lez\": \"v0.1.2/cf3639d8252040d13b3d4e933feb19b42c76e14a\","
    print "    \"risc0\": \"3.0.5\","
    print "    \"elf_sha256\": \"a324355c6417f6ac7265ab8ba880287d0976e8c27a672917d293bddd80be7006\","
    print "    \"image_id\": \"c14c978abbaedeffb54c71aa6a96275d1fdb66fcf79f7343bf6bf7aee04f4483\","
    print "    \"model\": \"production V03State transition; mandatory Clock excluded; chained calls included\""
    print "  },"
    print "  \"operations\": ["
    for (i = 1; i <= expected_count; i++) {
        operation = expected[i]
        print "    {"
        print "      \"name\": \"" operation "\","
        print "      \"sessions\": ["
        for (session = 1; session <= 2; session++) {
            role = session == 1 ? "escrow_root" : "authenticated_transfer_child"
            print "        {"
            print "          \"position\": " session ","
            print "          \"role\": \"" role "\","
            print "          \"segments\": " segments[operation, session] ","
            print "          \"total_cycles\": " total[operation, session] ","
            print "          \"user_cycles\": " user[operation, session] ","
            print "          \"paging_cycles\": " paging[operation, session] ","
            print "          \"reserved_cycles\": " reserved[operation, session]
            print session == 2 ? "        }" : "        },"
        }
        print "      ],"
        print "      \"recursive_total_cycles\": 655360,"
        print "      \"recursive_user_cycles\": " recursive_user[operation] ","
        print "      \"recursive_user_cycle_budget\": " budget[operation]
        print i == expected_count ? "    }" : "    },"
    }
    print "  ]"
    print "}"
}
