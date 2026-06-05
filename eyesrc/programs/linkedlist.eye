-- BROKEN (left to fix later): the `Node* next` self-reference compiles fine -
-- the type topology pass handles pointer cycles. What fails is the sentinel's
-- self-referential *initializer* `Node { ..., next: &sentinel }`: name
-- resolution rejects `sentinel` inside its own declaration. Because Eye has no
-- null and demands valid-by-construction init, a node cannot point at itself at
-- birth. Fix needs either two-phase init or a null/uninit escape - neither is
-- in the kernel yet.

structure Node {
    int32 value,
    Node* next, -- Perfectly legal now thanks to the topological sort!
};

-- Traverses the list and prints values until it hits the sentinel node
print_list(Node* head, Node* sentinel) {
    mut Node* current = head;

    loop {
        -- Address equality check: Stop when we point to the sentinel
        if current == sentinel { break; }

        println("Node Value: {}\n", current.value);
        current = current.next; -- Move to the next node
    }
}

main() {
    -- 1. Create the sentinel node.
    -- To keep it valid by construction, it initially points to itself.
    mut Node sentinel = Node { value: -1, next: &sentinel };

    -- 2. Build the list on the stack.
    -- We link them backwards so each node can receive a valid address.
    mut Node third  = Node { value: 30, next: &sentinel };
    mut Node second = Node { value: 20, next: &third };
    mut Node first  = Node { value: 10, next: &second };

    -- 3. Traverse our self-referential graph safely
    print_list(&first, &sentinel);
}
