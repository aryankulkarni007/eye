-- eye sandbox

extern {
    rand() -> int32;
}

enum Coin = Head | Tail;

structure Human {
    string name,
    -- [int32; 4] relations,
    int32 age,
};


decide(Coin coin) -> bool {
    match coin {
        Head -> true,
        Tail -> false,
    }
}

print_decision(bool decision) {
    if decision {
        print("yes");
    } else {
        print("no");
    }
}


main() {
    mut int32 decision = rand();
    if decision >= 1 {
        decision = 1;
    } else if decision <= 0 {
        decision = 0;
    }
    mut Coin coin = Head;
    if decision == 0 {
        coin = Tail
    } else if decision == 1 {
        coin = Head
    }
    print_decision(decide(coin));
}

