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
    expected_count = 14
    expected[1] = "initialize_native_claim"
    expected[2] = "fund_native_claim"
    expected[3] = "claim_native"
    expected[4] = "initialize_native_refund"
    expected[5] = "fund_native_refund"
    expected[6] = "refund_native"
    expected[7] = "initialize_token_claim"
    expected[8] = "create_token_custody_claim"
    expected[9] = "fund_token_claim"
    expected[10] = "claim_token"
    expected[11] = "initialize_token_refund"
    expected[12] = "create_token_custody_refund"
    expected[13] = "fund_token_refund"
    expected[14] = "refund_token"

    for (i = 1; i <= 6; i++) expected_sessions[expected[i]] = 2
    expected_sessions["initialize_token_claim"] = 1
    expected_sessions["create_token_custody_claim"] = 3
    expected_sessions["fund_token_claim"] = 3
    expected_sessions["claim_token"] = 3
    expected_sessions["initialize_token_refund"] = 1
    expected_sessions["create_token_custody_refund"] = 3
    expected_sessions["fund_token_refund"] = 3
    expected_sessions["refund_token"] = 3

    for (i = 1; i <= expected_count; i++) {
        operation = expected[i]
        expected_total[operation, 1] = operation ~ /^(fund_token|claim_token|refund_token)/ ? 1048576 : 524288
        role[operation, 1] = "escrow_root"
    }
    for (i = 1; i <= 6; i++) {
        operation = expected[i]
        expected_total[operation, 2] = 131072
        role[operation, 2] = "authenticated_transfer_child"
    }
    for (i = 8; i <= 14; i++) {
        operation = expected[i]
        if (expected_sessions[operation] == 3) {
            expected_total[operation, 2] = 524288
            expected_total[operation, 3] = 262144
            role[operation, 2] = "ata_child"
            role[operation, 3] = "token_grandchild"
        }
    }

    budget["initialize_native_claim"] = 375000
    budget["fund_native_claim"] = 460000
    budget["claim_native"] = 490000
    budget["initialize_native_refund"] = 375000
    budget["fund_native_refund"] = 460000
    budget["refund_native"] = 475000
    budget["initialize_token_claim"] = 305000
    budget["create_token_custody_claim"] = 950000
    budget["fund_token_claim"] = 860000
    budget["claim_token"] = 1120000
    budget["initialize_token_refund"] = 305000
    budget["create_token_custody_refund"] = 950000
    budget["fund_token_refund"] = 860000
    budget["refund_token"] = 1065000
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
    if (session != expected_sessions[operation]) {
        fail(operation " emitted " session " sessions; expected " expected_sessions[operation])
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
        recursive_cycles[operation] = 0
        recursive_user[operation] = 0
        for (session = 1; session <= expected_sessions[operation]; session++) {
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
            if (total[operation, session] != expected_total[operation, session]) {
                fail(operation " session " session " allocated-cycle regression")
            }
            recursive_cycles[operation] += total[operation, session]
            recursive_user[operation] += user[operation, session]
        }
        if (recursive_user[operation] > budget[operation]) {
            fail(operation " exceeds recursive user-cycle budget")
        }
    }

    print "{"
    print "  \"schema_version\": 1,"
    print "  \"measured_on\": \"2026-07-31\","
    print "  \"execution\": {"
    print "    \"lez\": \"v0.1.2/cf3639d8252040d13b3d4e933feb19b42c76e14a\","
    print "    \"risc0\": \"3.0.5\","
    print "    \"elf_sha256\": \"fe8ec1166ec886693d1fcd1d1ddc80090f81f6fab941851cce43b5bfb0c739f7\","
    print "    \"image_id\": \"5421868ee00d213bf083c09f14ed09f303e8581b95b3a17bb9b79f6cb44add62\","
    print "    \"model\": \"production V03State transition; mandatory Clock excluded; authenticated-transfer and ATA/Token recursion included\""
    print "  },"
    print "  \"operations\": ["
    for (i = 1; i <= expected_count; i++) {
        operation = expected[i]
        print "    {"
        print "      \"name\": \"" operation "\","
        print "      \"sessions\": ["
        for (session = 1; session <= expected_sessions[operation]; session++) {
            print "        {"
            print "          \"position\": " session ","
            print "          \"role\": \"" role[operation, session] "\","
            print "          \"segments\": " segments[operation, session] ","
            print "          \"total_cycles\": " total[operation, session] ","
            print "          \"user_cycles\": " user[operation, session] ","
            print "          \"paging_cycles\": " paging[operation, session] ","
            print "          \"reserved_cycles\": " reserved[operation, session]
            print session == expected_sessions[operation] ? "        }" : "        },"
        }
        print "      ],"
        print "      \"recursive_total_cycles\": " recursive_cycles[operation] ","
        print "      \"recursive_user_cycles\": " recursive_user[operation] ","
        print "      \"recursive_user_cycle_budget\": " budget[operation]
        print i == expected_count ? "    }" : "    },"
    }
    print "  ]"
    print "}"
}
