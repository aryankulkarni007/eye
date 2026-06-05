--* Mandelbrot Set ASCII Art Generator --*
--*
  *  Plots the Mandelbrot set in the terminal by computing the escape
  *  iteration for each pixel and mapping it to a character.  Uses
  *  libc's putchar(3) to output single characters without a newline,
  *  then uses println("") to advance to the next line.
--*

-- bring in libc's putchar so we can print without automatic newlines
extern {
    putchar(int32 c) -> int32;
}

-- A complex number: real + imaginary
structure Complex {
    float64 re,   -- real part
    float64 im,   -- imaginary part
};

-- Return the iteration count at which `c` escapes (or max_iter if it never does).
mandel(Complex c, int32 max_iter) -> int32 {
    mut float64 zr = 0.0;   -- real part of z
    mut float64 zi = 0.0;   -- imaginary part of z
    mut float64 tmp = 0.0;

    mut int32 iter = 0;
    loop {
        -- bail out if we've done enough iterations or escaped
        if iter >= max_iter { break; }
        if zr * zr + zi * zi > 4.0 { break; }

        -- z = z^2 + c
        tmp = zr * zr - zi * zi + c.re;
        zi  = 2.0 * zr * zi + c.im;
        zr  = tmp;

        iter = iter + 1;
    }

    iter   -- return the iteration count
}

main() {
    let int32   width   = 80;   -- columns (characters)
    let int32   height  = 40;   -- rows
    let int32   max_iters = 50; -- maximum iterations per pixel

    -- viewport in the complex plane
    let float64 xmin = -2.0;
    let float64 xmax =  1.0;
    let float64 ymin = -1.2;
    let float64 ymax =  1.2;

    let float64 dx = (xmax - xmin) / width  as float64;
    let float64 dy = (ymax - ymin) / height as float64;

    mut int32 py = 0;
    loop {
        if py >= height { break; }

        -- compute imaginary coordinate for this row
        let float64 im = ymax - (py as float64) * dy;

        mut int32 px = 0;
        loop {
            if px >= width { break; }

            -- compute real coordinate for this column
            let float64 re = xmin + (px as float64) * dx;

            let Complex c = Complex { re: re, im: im };
            let int32 its = mandel(c, max_iters);

            -- map iteration count to a character
            -- NOTE: this could become a match when we have match arm guards
            let char ch = if its == max_iters {
                '#'   -- in the set
            } else if its > 30 {
                '*'
            } else if its > 20 {
                '+'
            } else if its > 10 {
                '.'
            } else {
                ' '   -- very fast escape
            };

            -- output the character (no newline)
            putchar(ch as int32);

            px = px + 1;
        }

        println("");   -- just a newline (print always appends \n)

        py = py + 1;
    }
}
