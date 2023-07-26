# config/initializers/bullet.rb

if defined?(Bullet)
    Bullet.enable = true               # enable Bullet gem, otherwise do nothing
    Bullet.alert = false               # pop up a JavaScript alert in the browser
    Bullet.bullet_logger = false       # log to the Bullet log file (Bullet.log)
    Bullet.console = true              # log warnings to your browser's console.log (Safari/Firefox)
    Bullet.rails_logger = false        # add warnings directly to the Rails log
    Bullet.add_footer = false          # adds the details at the bottom of the page
  end