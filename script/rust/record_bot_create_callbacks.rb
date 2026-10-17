# Records every callback Rails runs when the new-bot wizard saves a Bots::DcaMultiAsset (plan P2-T7a), for
# rust/src/web/wizard/create.rs, which names how it handles each one (ported, or a no-op on create and why).
#   bin/rails runner script/rust/record_bot_create_callbacks.rb rust/tests/fixtures/bot_create_callbacks.json
# The chains are the class's own, with every concern's: after_initialize, validation (before/after_validation),
# validate (validate/validates), save, create and commit. A callback added, removed or moved in Rails, or a change to
# a file that defines one, makes the pin stale (script/rust/repin), and the Rust table must follow.
require 'json'
require 'digest'

klass = Bots::DcaMultiAsset
root = "#{Rails.root}/"
gem_dir = %r{\A#{Regexp.escape(Gem.dir)}/gems/}

where = lambda do |location|
  next nil unless location

  file, line = location
  "#{file.delete_prefix(root).sub(gem_dir, '')}:#{line}"
end

# The `on:` of a validation, a before/after_validation or a commit callback, read from the lambda Rails wraps it in.
context_of = lambda do |condition|
  binding = condition.binding
  %i[context fire_on options].each do |name|
    next unless binding.local_variable_defined?(name)

    value = binding.local_variable_get(name)
    value = value[:on] if name == :options
    return "on #{Array(value).map(&:to_s).sort.join(',')}"
  end
  nil
end

describe_condition = lambda do |condition|
  case condition
  when Symbol then condition.to_s
  when ActiveSupport::Callbacks::Conditionals::Value then 'the callback did not return false'
  when Proc
    location = where.(condition.source_location)
    location.start_with?('app/') ? "proc #{location}" : (context_of.(condition) || "proc #{location}")
  else condition.inspect
  end
end

describe_filter = lambda do |filter|
  case filter
  when Symbol then [filter.to_s, where.(klass.instance_method(filter).source_location)]
  when Proc then ["proc #{where.(filter.source_location)}", where.(filter.source_location)]
  when ActiveModel::Validator
    attributes = filter.respond_to?(:attributes) ? filter.attributes.map(&:to_s) : []
    options = filter.options.except(:if, :unless, :on).transform_keys(&:to_s).transform_values { |v| v.is_a?(Proc) ? 'proc' : v.as_json }
    ["#{filter.class.name}(#{attributes.join(', ')})#{" #{options.to_json}" unless options.empty?}", nil]
  else [filter.class.name, nil]
  end
end

sources = Set.new(%w[app/models/bot.rb app/models/bots/dca_multi_asset.rb])
chains = %w[initialize validation validate save create commit].to_h do |chain|
  callbacks = klass.public_send("_#{chain}_callbacks").map do |callback|
    name, defined_at = describe_filter.(callback.filter)
    conditions = callback.instance_variable_get(:@if).map(&describe_condition)
    exclusions = callback.instance_variable_get(:@unless).map(&describe_condition)
    [defined_at, *conditions, *exclusions].compact.each do |text|
      file = text.delete_prefix('proc ').split(':').first
      sources << file if file.start_with?('app/')
    end
    { 'kind' => callback.kind.to_s, 'callback' => name, 'defined_at' => defined_at, 'if' => conditions, 'unless' => exclusions }.compact
  end
  [chain, callbacks]
end

out = {
  'sources' => sources.sort.to_h { |file| [file, Digest::SHA256.file(Rails.root.join(file)).hexdigest] },
  'gems' => %w[activemodel activerecord activesupport].to_h { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] },
  'class' => klass.name,
  'callbacks' => chains
}
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{ARGV[0]}: #{chains.values.sum(&:size)} callbacks"
