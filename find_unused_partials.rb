#!/usr/bin/env ruby

require 'find'

def find_partials(root_path)
  partials = []
  Find.find(root_path) do |path|
    next unless path.end_with?('.html.erb', '.html.haml', '.html.slim')
    next unless File.basename(path).start_with?('_')

    partials << path
  end
  partials
end

def find_references(root_path, partial_name)
  references = []
  Find.find(root_path) do |path|
    next if File.directory?(path)
    next if path.end_with?('.log', '.git', '.jpg', '.png', '.gif')

    begin
      content = File.read(path)
      references << path if content.include?(partial_name)
    rescue StandardError
      puts "Error reading file: #{path}"
    end
  end
  references
end

# Set the root path of your Rails project
root_path = '.'

# Find all partials
partials = find_partials(root_path)

puts "Analyzing #{partials.length} partials...\n\n"

# Check each partial for references
partials.each do |partial_path|
  partial_name = File.basename(partial_path, '.*').gsub(/^_/, '')
  references = find_references(root_path, partial_name)

  if references.empty?
    puts "Potentially unused partial: #{partial_path}"
  else
    puts "#{partial_path} is referenced in #{references.length} files"
    references.each { |ref| puts "  - #{ref}" }
  end
  puts
end
