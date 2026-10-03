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
  'layouts/svg_mi_visibility-off' => 'svg/_mi_visibility_off.html',
  'svg/24x24_dot_menu' => 'svg/_24x24_dot_menu.html',
  'svg/24x24_reverse' => 'svg/_24x24_reverse.html',
  'svg/24x24_info' => 'svg/_24x24_info.html',
  'svg/24x24_info-filled' => 'svg/_24x24_info_filled.html',
  'svg/landscape_empty' => 'svg/_landscape_empty.html'
}.merge(
  # One mark per exchange class (Exchange#name_id), for the tiles and the exchange menu.
  Dir[Rails.root.join('app/views/svg/_exchange-*.html.erb')].sort.to_h do |path|
    name = File.basename(path, '.html.erb').delete_prefix('_')
    ["svg/#{name}", "svg/_#{name.tr('-', '_')}.html"]
  end
).freeze

PARTIALS.each do |partial, file|
  path = Rails.root.join('rust/templates', file)
  FileUtils.mkdir_p(path.dirname)
  File.write(path, ApplicationController.render(partial:))
  puts "wrote rust/templates/#{file}"
end
