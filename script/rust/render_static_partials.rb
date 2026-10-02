# Renders the Rails partials that take no data (icons) into rust/templates/, so the Rust templates
# include Rails' own markup instead of a hand copy:
#   bin/rails runner script/rust/render_static_partials.rb
# Re-run when one of these partials changes; rust/tests/pages.rs fails until the output is committed.
PARTIALS = {
  'svg/40x30_logo' => 'svg/_40x30_logo.html',
  'svg/24x24_close' => 'svg/_24x24_close.html',
  'svg/24x24_plus' => 'svg/_24x24_plus.html',
  'svg/24x24_sliders' => 'svg/_24x24_sliders.html',
  'svg/24x24_withdrawal' => 'svg/_24x24_withdrawal.html',
  'svg/16x16_down' => 'svg/_16x16_down.html',
  'layouts/svg_mi_visibility' => 'svg/_mi_visibility.html',
  'layouts/svg_mi_visibility-off' => 'svg/_mi_visibility_off.html'
}.freeze

PARTIALS.each do |partial, file|
  path = Rails.root.join('rust/templates', file)
  FileUtils.mkdir_p(path.dirname)
  File.write(path, ApplicationController.render(partial:))
  puts "wrote rust/templates/#{file}"
end
