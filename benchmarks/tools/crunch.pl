#!/usr/bin/perl
use common::sense;
use Data::Dumper;

# Generates gnuplot instructions to make a graph from the benchmarks.
# * Run benchmarks, saving with --logfile $i.log (for each measured parameter $i)
# * Run crunch.pl | gnuplot

my %data;

for my $f (<*.log>) {
	open my $in, '<', $f or die "Couldn't read $f: $!\n";

	$f =~ s/\..*//;

	for (<$in>) {
		s/^\s*//;
		s/,//g;
		my ($time, $name) = (/(\d+).* (\w+)/);
		$data{$name}->{$f} = $time;
	}
}

while (my ($server, $data) = each %data) {
	open my $out, '>', "$server.dat" or die "Couldn't write $server.dat: $!\n";
	for my $size (sort { $a <=> $b } keys %$data) {
		print $out "$size\t$data->{$size}\n";
	}
}

$\ = ";\n";
print "set terminal svg size 1024, 768 background rgb 'white'";
print "set output 'graph.svg'";
print "set log xyz";
print "set key right bottom";

my @colors = qw(red blue black orchid green brown purple olivegreen orange #83ffd5 #007f00 #8a0000);
my $cnum;

sub conv($) {
	my ($name) = @_;
	$name =~ s/_/\\_/g;
	return $name;
}

print "plot " . join ', ', (map "'$_.dat' title '".conv($_)."' with linespoints lt 1 lc rgb \"".$colors[$cnum ++ % scalar @colors]."\"", qw(async_cpus async_cpupool corona_cpus corona_blocking_wrapper_cpus futures_cpus futures_cpupool may threads));
