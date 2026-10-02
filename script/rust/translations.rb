# Every text Rails loads from config/locales, flattened to "<locale>.<key>" => text, for rust/tests/i18n.rs:
#   bin/rails runner script/rust/translations.rb <out.json>
# A fresh backend, so only this app's files are read (gems add their own to I18n's global backend).
# An array adds its index to the key, as rust/build.rs does.
require 'json'
backend = I18n::Backend::Simple.new
backend.load_translations(*Dir[Rails.root.join('config/locales/*.yml')].sort)
flat = {}
walk = lambda do |prefix, node|
  case node
  when Hash then node.each { |key, value| walk.("#{prefix}.#{key}", value) }
  when Array then node.each_with_index { |value, index| walk.("#{prefix}.#{index}", value) }
  else flat[prefix] = node
  end
end
backend.send(:translations).each { |locale, tree| walk.(locale.to_s, tree) }
File.write(ARGV.fetch(0), JSON.generate(flat))
puts "wrote #{flat.size} texts"
