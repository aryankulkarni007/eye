-- demo of eyelang's capabilities

structure Vector2 {
    int32 x,
    int32 y,
};

structure Particle {
    Vector2 position,
    Vector2 velocity,
    bool is_active,
};

add_vectors(Vector2 a, Vector2 b) -> Vector2 {
    Vector2 {
        x: a.x + b.x,
        y: a.y + b.y
    }
}

main() {
    -- Configure bounds and environment constants
    const int32 max_height = 100;
    const int32 gravity_y = 2;
    var int32 frame_count = 0;

    -- Instantiate our particle using nested struct literal initialization
    var Particle p = Particle {
        position: Vector2 { x: 10, y: 0 },
        velocity: Vector2 { x: 5, y: 10 },
        is_active: true
    };

    -- Get a reference to mutate our particle state ergonomically using '.'
    var &Particle p_ref = &p;

    print("Simulating particle physics setup...");

    loop {
        -- Check simulation termination condition
        if frame_count > 5 {
            break;
        }

        -- Apply gravity to velocity
        p_ref.velocity.y = p_ref.velocity.y + gravity_y;

        -- Update position using our vector addition helper function
        p_ref.position = add_vectors(p_ref.position, p_ref.velocity);

        -- Modern expression assignment: cap height at boundary max_height
        const int32 current_y = p_ref.position.y;
        p_ref.position.y = if current_y > max_height {
            max_height
        } else {
            current_y
        };

        -- Check if particle hit the ground floor
        const bool hit_ground = p_ref.position.y == max_height;
        if hit_ground {
            p_ref.velocity.y = 0;
            p_ref.is_active = false;
        }

        -- Print out telemetry data using your print formatting rules
        print("Frame {}: Position({}, {}) | Active: {}",
            frame_count,
            p_ref.position.x,
            p_ref.position.y,
            p_ref.is_active
        );

        frame_count = frame_count + 1;
    }
}
