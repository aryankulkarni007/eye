-- Sieve of Eratosthenes: print every prime below LIMIT. The `composite` flags
-- start all-false via the repeat array literal `[false; 50]` (an N-element
-- array filled in one expression). The marking loops strike each prime's
-- multiples; the survivors are the primes.


main() {
    const usize LIMIT = 50;

    -- composite[i] becomes true once i is known to be non-prime. starts false.
    mut [bool; 50] composite = [false; 50];

    -- mark multiples: for each prime i, strike i*i, i*i+i, i*i+2i, ...
    mut usize i = 2;
    loop {
        if i * i >= LIMIT { break; }
        if !composite[i] {
            mut usize j = i * i;
            loop {
                if j >= LIMIT { break; }
                composite[j] = true;
                j = j + i;
            }
        }
        i = i + 1;
    }

    -- the survivors (index >= 2, never marked) are the primes.
    mut usize k = 2;
    loop {
        if k >= LIMIT { break; }
        if !composite[k] {
            println("{}", k);
        }
        k = k + 1;
    }
}
